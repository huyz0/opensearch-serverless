//! A shard's segment merging on this node: the setting that turns it on and
//! the gate each shard asks before it merges.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use oss_mem::{MemoryBudget, Pool, Reservation};
use oss_types::settings::{BoolSetting, LongSetting, Scope, Setting};
use oss_writer::index::{MergeGate, MergeGuard, MergeSettings};

use super::WriterRole;

/// `serverless.writer.merge.enabled`: whether a shard merges its small segments after a refresh.
pub const ENABLED: BoolSetting = BoolSetting::new(
    "serverless.writer.merge.enabled",
    true,
    Scope::Node,
    "Whether a shard this node writes merges its small segments after a refresh, so a commit lists \
     a bounded number of segments. On by default: it makes no request, and a cold open and a refresh get cheaper (ADR 0059).",
);

/// `serverless.writer.merge.segments_per_tier`.
pub const SEGMENTS_PER_TIER: LongSetting = LongSetting::new(
    "serverless.writer.merge.segments_per_tier",
    8,
    2,
    64,
    Scope::Node,
    "How many segments one tier of a shard holds before a merge is due.",
);

/// `serverless.writer.merge.max_merge_at_once`.
pub const MAX_MERGE_AT_ONCE: LongSetting = LongSetting::new(
    "serverless.writer.merge.max_merge_at_once",
    10,
    2,
    64,
    Scope::Node,
    "How many segments one merge takes at most.",
);

/// `serverless.writer.merge.max_merged_segment_mb`.
pub const MAX_MERGED_SEGMENT_MB: LongSetting = LongSetting::new(
    "serverless.writer.merge.max_merged_segment_mb",
    64,
    1,
    1024,
    Scope::Node,
    "The size in MiB past which a segment is not merged again, and so the most one merge writes; \
     a merge reserves twice this from the open-indexes pool at most.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    ENABLED.setting(),
    SEGMENTS_PER_TIER.setting(),
    MAX_MERGE_AT_ONCE.setting(),
    MAX_MERGED_SEGMENT_MB.setting(),
];

/// `OSS_WRITER_MERGE`, `OSS_WRITER_MERGE_SEGMENTS_PER_TIER`,
/// `OSS_WRITER_MERGE_AT_ONCE`, `OSS_WRITER_MERGE_MAX_SEGMENT_MB`: the
/// `serverless.writer.merge.*` settings of this node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeConfig {
    /// Whether shards merge their small segments; on by default.
    pub enabled: bool,
    /// Segments one tier holds before a merge is due.
    pub segments_per_tier: usize,
    /// Segments one merge takes at most.
    pub max_merge_at_once: usize,
    /// The size past which a segment is not merged, in bytes.
    pub max_merged_segment_bytes: u64,
}

impl MergeConfig {
    /// Merging off, every bound at its default.
    pub fn off() -> Self {
        Self {
            enabled: false,
            segments_per_tier: 8,
            max_merge_at_once: 10,
            max_merged_segment_bytes: 64 << 20,
        }
    }

    /// Reads the settings from `get`, each bounded.
    ///
    /// # Errors
    ///
    /// A value that is not a number or not true or false, or is out of its bounds.
    pub fn read(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let text = |key: &str, default: &str| {
            get(key)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_owned())
        };
        let number = |key: &str, default: &str, low: u64, high: u64| -> Result<u64, String> {
            let value: u64 = text(key, default)
                .parse()
                .map_err(|e| format!("{key}: {e}"))?;
            if (low..=high).contains(&value) {
                Ok(value)
            } else {
                Err(format!("{key} [{value}] is outside {low}..={high}"))
            }
        };
        let enabled = match text("OSS_WRITER_MERGE", "true").as_str() {
            "true" => true,
            "false" => false,
            other => return Err(format!("OSS_WRITER_MERGE is [{other}], not true or false")),
        };
        Ok(Self {
            enabled,
            segments_per_tier: usize::try_from(number(
                "OSS_WRITER_MERGE_SEGMENTS_PER_TIER",
                "8",
                2,
                64,
            )?)
            .unwrap_or(8),
            max_merge_at_once: usize::try_from(number("OSS_WRITER_MERGE_AT_ONCE", "10", 2, 64)?)
                .unwrap_or(10),
            max_merged_segment_bytes: number("OSS_WRITER_MERGE_MAX_SEGMENT_MB", "64", 1, 1024)?
                << 20,
        })
    }
}

/// What a node grants a shard that asks to merge: one merge at a time, and
/// the bytes reserved from the open-indexes pool.
pub(crate) struct NodeMergeGate {
    budget: MemoryBudget,
    busy: Arc<AtomicBool>,
    skipped_for_permit: AtomicU64,
    skipped_for_memory: AtomicU64,
    started: AtomicU64,
}

/// Held while a merge runs; gives the permit and the bytes back.
struct Held {
    busy: Arc<AtomicBool>,
    reserved: Option<Reservation>,
}

impl Drop for Held {
    fn drop(&mut self) {
        // The bytes go back before the permit, so the next shard that gets
        // the permit finds them free.
        drop(self.reserved.take());
        self.busy.store(false, Ordering::Release);
    }
}

impl NodeMergeGate {
    pub(crate) fn new(budget: MemoryBudget) -> Self {
        Self {
            budget,
            busy: Arc::new(AtomicBool::new(false)),
            skipped_for_permit: AtomicU64::new(0),
            skipped_for_memory: AtomicU64::new(0),
            started: AtomicU64::new(0),
        }
    }

    /// How many merges were skipped because another shard was merging, and
    /// because the pool could not hold the merge.
    pub(crate) fn skipped(&self) -> (u64, u64) {
        (
            self.skipped_for_permit.load(Ordering::Relaxed),
            self.skipped_for_memory.load(Ordering::Relaxed),
        )
    }
}

impl MergeGate for NodeMergeGate {
    fn try_start(&self, bytes: u64) -> Option<Box<dyn MergeGuard>> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.skipped_for_permit.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let wanted = usize::try_from(bytes).unwrap_or(usize::MAX);
        match self.budget.try_reserve(Pool::OpenIndexes, wanted) {
            // ALLOC: one guard a merge.
            Ok(reserved) => {
                self.started.fetch_add(1, Ordering::Relaxed);
                Some(Box::new(Held {
                    busy: Arc::clone(&self.busy),
                    reserved: Some(reserved),
                }))
            }
            Err(_) => {
                self.busy.store(false, Ordering::Release);
                self.skipped_for_memory.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
}

impl WriterRole {
    /// Makes the shards this role opens from now on merge their small
    /// segments under `config`, drawing their memory from `budget`. Nothing
    /// changes when `config` is off.
    pub fn set_merging(&self, config: &MergeConfig, budget: &MemoryBudget) {
        if !config.enabled {
            return;
        }
        let settings = MergeSettings {
            segments_per_tier: config.segments_per_tier,
            max_merge_at_once: config.max_merge_at_once,
            max_merged_segment_bytes: config.max_merged_segment_bytes,
        };
        let gate = Arc::new(NodeMergeGate::new(budget.clone()));
        // A second call keeps the first.
        let _ = self.merging.set((settings, gate));
    }
}

impl WriterRole {
    /// What a shard opened now merges with, when merging is on.
    pub(super) fn merging_for_open(&self) -> Option<(MergeSettings, Arc<dyn MergeGate>)> {
        let (settings, gate) = self.merging.get()?;
        let gate: Arc<dyn MergeGate> = Arc::clone(gate) as Arc<dyn MergeGate>;
        Some((settings.clone(), gate))
    }

    /// How many merges this node's shards started.
    pub fn merges_started(&self) -> u64 {
        self.merging
            .get()
            .map_or(0, |(_, gate)| gate.started.load(Ordering::Relaxed))
    }

    /// How many merges were skipped because another shard of this node was
    /// merging, and because the memory pool could not hold the merge.
    pub fn merges_skipped(&self) -> (u64, u64) {
        self.merging
            .get()
            .map_or((0, 0), |(_, gate)| gate.skipped())
    }
}

#[cfg(test)]
mod tests {
    use oss_mem::BudgetConfig;

    use super::*;

    fn budget(open_indexes: usize) -> MemoryBudget {
        MemoryBudget::new(
            &BudgetConfig::new(open_indexes.saturating_mul(2))
                .with_pool(Pool::OpenIndexes, open_indexes)
                .expect("fits"),
        )
    }

    fn get<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    // docs/specs/writer-segment-merge AC-4
    #[test]
    fn one_merge_runs_at_a_time_and_the_next_is_refused_not_waited_for() {
        let gate = NodeMergeGate::new(budget(1 << 20));
        let first = gate.try_start(1024).expect("granted");
        assert!(gate.try_start(1024).is_none(), "a second is refused");
        assert_eq!(gate.skipped(), (1, 0));
        drop(first);
        assert!(gate.try_start(1024).is_some(), "the permit came back");
    }

    // docs/specs/writer-segment-merge AC-4
    #[test]
    fn a_merge_the_pool_cannot_hold_is_refused_and_gives_the_permit_back() {
        let gate = NodeMergeGate::new(budget(1024));
        assert!(gate.try_start(1 << 20).is_none());
        assert_eq!(gate.skipped(), (0, 1));
        assert!(gate.try_start(512).is_some(), "the permit was not kept");
    }

    // docs/specs/writer-segment-merge AC-4
    #[test]
    fn the_reservation_is_given_back_when_the_guard_drops() {
        let budget = budget(4096);
        let gate = NodeMergeGate::new(budget.clone());
        let held = gate.try_start(4096).expect("granted");
        assert_eq!(budget.in_use(Pool::OpenIndexes), 4096);
        drop(held);
        assert_eq!(budget.in_use(Pool::OpenIndexes), 0);
    }

    // docs/specs/writer-segment-merge AC-5, AC-12
    #[test]
    fn merging_is_on_by_default_off_by_its_variable_and_each_bound_is_read_and_checked() {
        let on = MergeConfig::read(&get(&[])).expect("defaults");
        assert!(on.enabled);
        assert_eq!(
            MergeConfig {
                enabled: false,
                ..on
            },
            MergeConfig::off()
        );
        let off = MergeConfig::read(&get(&[("OSS_WRITER_MERGE", "false")])).expect("read");
        assert_eq!(off, MergeConfig::off());
        let on = MergeConfig::read(&get(&[
            ("OSS_WRITER_MERGE", "true"),
            ("OSS_WRITER_MERGE_SEGMENTS_PER_TIER", "4"),
            ("OSS_WRITER_MERGE_AT_ONCE", "6"),
            ("OSS_WRITER_MERGE_MAX_SEGMENT_MB", "16"),
        ]))
        .expect("read");
        assert_eq!(
            (on.enabled, on.segments_per_tier, on.max_merge_at_once),
            (true, 4, 6)
        );
        assert_eq!(on.max_merged_segment_bytes, 16 << 20);
        for (key, value) in [
            ("OSS_WRITER_MERGE", "yes"),
            ("OSS_WRITER_MERGE_SEGMENTS_PER_TIER", "1"),
            ("OSS_WRITER_MERGE_AT_ONCE", "65"),
            ("OSS_WRITER_MERGE_MAX_SEGMENT_MB", "0"),
            ("OSS_WRITER_MERGE_MAX_SEGMENT_MB", "many"),
        ] {
            assert!(MergeConfig::read(&get(&[(key, value)])).is_err(), "{key}");
        }
    }
}
