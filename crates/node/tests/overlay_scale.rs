//! docs/specs/tail-overlay-scale: what a reader's tail overlay costs a node
//! that follows thousands of shards: every shard tracked, `RECORDS` pushed
//! writes held for it, and an overlay built over them by the one builder
//! worker, charged to the `open_indexes` pool. Evidence run by hand with
//! `--ignored --nocapture`; a time is not a predicate, so the assertions are
//! the counted ones (the pool is never overdrawn, a refused build serves the
//! commit).

mod overlay_rig;

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_mem::Pool;
use oss_mem::{BudgetConfig, MemoryBudget};
use oss_node::overlay_builder::OverlayBuilder;
use oss_node::tail_held::TailHeld;
use oss_reader::overlay::OverlaySource;
use overlay_rig::{commit, index, stage};

// ARITH: test arithmetic on counts, bytes and durations; an overflow panics the test.
#[allow(clippy::arithmetic_side_effects, clippy::cast_precision_loss)]
mod run {
    use super::*;

    const RECORDS: i64 = 20;

    /// The process's resident set in bytes; zero where there is none.
    fn rss() -> u64 {
        use std::io::Read;
        let mut text = [0_u8; 128];
        let Ok(mut file) = std::fs::File::open("/proc/self/statm") else {
            return 0;
        };
        let read = file.read(&mut text).unwrap_or(0);
        std::str::from_utf8(&text[..read])
            .ok()
            .and_then(|s| {
                s.split_whitespace()
                    .nth(1)
                    .and_then(|p| p.parse::<u64>().ok())
            })
            .map_or(0, |pages| pages * 4096)
    }

    pub fn one(shards: u32, pool: usize) {
        let held = Arc::new(TailHeld::new());
        let config = BudgetConfig::new(1 << 30)
            .with_pool(Pool::OpenIndexes, pool)
            .expect("fits");
        let budget = MemoryBudget::new(&config);
        let builder = OverlayBuilder::new(Arc::clone(&held), budget.clone());
        let commit = commit(&["a", "b", "c"]);
        let before = rss();
        let started = Instant::now();
        // Every shard is tracked by its first ask and given its records.
        for i in 0..shards {
            let uuid = format!("uuid-{i:06}");
            assert!(builder.ready(&uuid, 0, &stage(1), &commit, 1_000).is_none());
            let records: Vec<_> = (0..RECORDS)
                .map(|n| index(&format!("d{i}-{n}"), n, n + 1))
                .collect();
            held.take(&uuid, 0, &records, 1_000);
        }
        let held_at = rss().saturating_sub(before);
        // Searches ask again until each shard's overlay is built or refused.
        let mut built = 0_u32;
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            built = 0;
            for i in 0..shards {
                let uuid = format!("uuid-{i:06}");
                if builder.ready(&uuid, 0, &stage(1), &commit, 1_000).is_some() {
                    built += 1;
                }
            }
            if builder.pending() == 0 && builder.settle() {
                break;
            }
        }
        let total = started.elapsed();
        let in_use = budget.in_use(Pool::OpenIndexes);
        let all = rss().saturating_sub(before);
        println!(
            "shards={shards:>6} pool={:>4} MiB held-rss+={:>6} KiB ({:>5} B/shard) built={built} \
             builds={} charged={} KiB rss+={} KiB ({} B/overlay) total={total:?} ({:.0} us/shard)",
            pool >> 20,
            held_at / 1024,
            held_at / u64::from(shards),
            builder.builds(),
            in_use / 1024,
            all / 1024,
            all.saturating_sub(held_at) / u64::from(built.max(1)),
            total.as_secs_f64() * 1e6 / f64::from(shards),
        );
        assert!(in_use <= pool, "the pool is never overdrawn");
    }
}

#[test]
#[ignore = "evidence: run by hand with --nocapture"]
fn the_readers_overlay_at_thousands_of_shards() {
    for shards in [100, 1_000, 5_000] {
        run::one(shards, 256 << 20);
    }
    run::one(5_000, 4 << 20);
}
