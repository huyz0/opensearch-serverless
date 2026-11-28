//! The `OSS_PAGE_CACHE_*` variables: the page cache's disk tier.
//!
//! Without a directory the cache keeps nothing past a read, as it always did.

use std::path::PathBuf;

use super::write_limit::Bounded;

/// The most a tier may be given: 1 PiB, far past any disk and far under what
/// its page records could address.
const MAX_BYTES: u64 = 1 << 50;

/// A disk tier for the page cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskCache {
    /// The directory the tier lives in; emptied when the node starts.
    pub dir: PathBuf,
    /// The most bytes of slots it holds.
    pub bytes: u64,
    /// What this node answers a joining reader with; none, and it answers
    /// nothing.
    pub warm: Option<WarmPeer>,
}

/// What bounds the pages this node tells a joining reader of and serves it
/// (`OSS_WARM_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarmPeer {
    /// How many of a shard's first readers a joiner must be among.
    pub width: u64,
    /// The most shards one hot-set answer names.
    pub max_shards: u64,
    /// The most stored bytes one hot-set answer names.
    pub max_bytes: u64,
    /// The most warm requests served at once.
    pub max_concurrent: u64,
    /// The percent of the bytes named a joining node holds before it is
    /// ready.
    pub ready_percent: u64,
    /// The seconds a joining node waits for that, at most 5 minutes.
    pub max_wait_secs: u64,
    /// The most bytes a second a joining node takes.
    pub bytes_per_second: u64,
    /// The most peers a joining node takes from at once.
    pub parallel_peers: u64,
    /// The most of a peer's shards a joining node opens at once.
    pub open_parallel: u64,
    /// The most bytes a second a peer serves to joining nodes, across all of
    /// them.
    pub serve_bytes_per_second: u64,
}

/// The longest a joining node is held out of the readers, in seconds.
pub const MAX_WAIT_CAP_SECS: u64 = 300;

/// Reads the tier: `dir` is `OSS_PAGE_CACHE_DIR` (empty for none); a
/// directory needs `OSS_PAGE_CACHE_BYTES`, a size at least one page.
/// `OSS_WARM_ON_JOIN` (default false) makes the node answer a joining
/// reader, which needs the tier to have pages to answer with.
pub(in crate::boot) fn read(
    dir: &str,
    bounded: Bounded<'_>,
    flag: &dyn Fn(&str, &str) -> Result<bool, String>,
) -> Result<Option<DiskCache>, String> {
    let warm = flag("OSS_WARM_ON_JOIN", "false")?;
    if dir.is_empty() {
        if warm {
            return Err("OSS_WARM_ON_JOIN needs OSS_PAGE_CACHE_DIR".to_owned());
        }
        return Ok(None);
    }
    let bytes = bounded("OSS_PAGE_CACHE_BYTES", "0", 0, MAX_BYTES)?;
    if bytes < 1 << 20 {
        return Err(format!(
            "OSS_PAGE_CACHE_DIR is set, so OSS_PAGE_CACHE_BYTES [{bytes}] must give the tier at least 1 MiB"
        ));
    }
    let warm = if warm {
        Some(WarmPeer {
            width: bounded("OSS_WARM_WIDTH", "2", 1, 64)?,
            max_shards: bounded("OSS_WARM_MAX_SHARDS", "1000", 1, 1_000_000)?,
            max_bytes: bounded("OSS_WARM_MAX_BYTES", "8589934592", 1, MAX_BYTES)?,
            max_concurrent: bounded("OSS_WARM_MAX_CONCURRENT", "2", 1, 64)?,
            ready_percent: bounded("OSS_WARM_READY_PERCENT", "90", 1, 100)?,
            max_wait_secs: bounded("OSS_WARM_MAX_WAIT_SECONDS", "60", 1, 86_400)?
                .min(MAX_WAIT_CAP_SECS),
            bytes_per_second: bounded("OSS_WARM_BYTES_PER_SECOND", "268435456", 1, MAX_BYTES)?,
            parallel_peers: bounded("OSS_WARM_PARALLEL_PEERS", "4", 1, 64)?,
            open_parallel: bounded("OSS_WARM_OPEN_PARALLEL", "8", 1, 64)?,
            serve_bytes_per_second: bounded(
                "OSS_WARM_SERVE_BYTES_PER_SECOND",
                "134217728",
                1,
                MAX_BYTES,
            )?,
        })
    } else {
        None
    };
    Ok(Some(DiskCache {
        dir: PathBuf::from(dir),
        bytes,
        warm,
    }))
}

/// The least a resident bound may be: below it the pager's eviction batches
/// outweigh the bound.
const MIN_RESIDENT_BYTES: u64 = 1 << 20;

/// Reads `OSS_RESIDENT_PAGE_BYTES`: 0 (the default) leaves the engine's
/// bound, otherwise at least 1 MiB.
pub(in crate::boot) fn read_resident(bounded: Bounded<'_>) -> Result<u64, String> {
    let bytes = bounded("OSS_RESIDENT_PAGE_BYTES", "0", 0, MAX_BYTES)?;
    if bytes != 0 && bytes < MIN_RESIDENT_BYTES {
        return Err(format!(
            "OSS_RESIDENT_PAGE_BYTES [{bytes}] must be 0 or at least 1 MiB"
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn read_with(vars: &[(&str, &str)]) -> Result<Option<DiskCache>, String> {
        let vars: BTreeMap<&str, &str> = vars.iter().copied().collect();
        let get = |key: &str, default: &str| vars.get(key).copied().unwrap_or(default).to_owned();
        let bounded = |key: &str, default: &str, low: u64, high: u64| {
            let value: u64 = get(key, default)
                .parse()
                .map_err(|e| format!("{key}: {e}"))?;
            if (low..=high).contains(&value) {
                Ok(value)
            } else {
                Err(format!("{key} [{value}] is not within {low} to {high}"))
            }
        };
        let flag = |key: &str, default: &str| match get(key, default).as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!("{key} is [{other}], not true or false")),
        };
        read(get("OSS_PAGE_CACHE_DIR", "").as_str(), &bounded, &flag)
    }

    const TIER: [(&str, &str); 2] = [
        ("OSS_PAGE_CACHE_DIR", "/tmp/tier"),
        ("OSS_PAGE_CACHE_BYTES", "67108864"),
    ];

    // docs/specs/reader-warm-copy AC-1
    #[test]
    fn the_warm_peer_is_off_unless_asked_for_and_then_has_its_defaults() {
        let off = read_with(&TIER).expect("reads").expect("a tier");
        assert_eq!(off.warm, None);

        let mut on = TIER.to_vec();
        on.push(("OSS_WARM_ON_JOIN", "true"));
        let warm = read_with(&on).expect("reads").expect("a tier").warm;
        assert_eq!(
            warm,
            Some(WarmPeer {
                width: 2,
                max_shards: 1000,
                max_bytes: 8 << 30,
                max_concurrent: 2,
                ready_percent: 90,
                max_wait_secs: 60,
                bytes_per_second: 256 << 20,
                parallel_peers: 4,
                open_parallel: 8,
                serve_bytes_per_second: 128 << 20,
            })
        );
    }

    #[test]
    fn the_warm_variables_are_read_and_held_to_their_ranges() {
        let mut vars = TIER.to_vec();
        vars.extend([
            ("OSS_WARM_ON_JOIN", "true"),
            ("OSS_WARM_WIDTH", "3"),
            ("OSS_WARM_MAX_SHARDS", "7"),
            ("OSS_WARM_MAX_BYTES", "1000"),
            ("OSS_WARM_MAX_CONCURRENT", "5"),
            ("OSS_WARM_READY_PERCENT", "75"),
            ("OSS_WARM_MAX_WAIT_SECONDS", "3600"),
            ("OSS_WARM_BYTES_PER_SECOND", "1000"),
            ("OSS_WARM_PARALLEL_PEERS", "3"),
            ("OSS_WARM_OPEN_PARALLEL", "5"),
            ("OSS_WARM_SERVE_BYTES_PER_SECOND", "2000"),
        ]);
        let warm = read_with(&vars).expect("reads").expect("tier").warm;
        assert_eq!(
            warm,
            Some(WarmPeer {
                width: 3,
                max_shards: 7,
                max_bytes: 1000,
                max_concurrent: 5,
                ready_percent: 75,
                max_wait_secs: 300,
                bytes_per_second: 1000,
                parallel_peers: 3,
                open_parallel: 5,
                serve_bytes_per_second: 2000,
            })
        );
        for (key, bad) in [
            ("OSS_WARM_WIDTH", "0"),
            ("OSS_WARM_MAX_SHARDS", "0"),
            ("OSS_WARM_MAX_BYTES", "0"),
            ("OSS_WARM_MAX_CONCURRENT", "65"),
            ("OSS_WARM_READY_PERCENT", "0"),
            ("OSS_WARM_READY_PERCENT", "101"),
            ("OSS_WARM_MAX_WAIT_SECONDS", "0"),
            ("OSS_WARM_BYTES_PER_SECOND", "0"),
            ("OSS_WARM_PARALLEL_PEERS", "65"),
            ("OSS_WARM_OPEN_PARALLEL", "0"),
            ("OSS_WARM_OPEN_PARALLEL", "65"),
            ("OSS_WARM_SERVE_BYTES_PER_SECOND", "0"),
        ] {
            let mut vars = TIER.to_vec();
            vars.extend([("OSS_WARM_ON_JOIN", "true"), (key, bad)]);
            assert!(read_with(&vars).is_err(), "{key}={bad}");
        }
    }

    #[test]
    fn warming_without_a_disk_tier_is_refused_and_a_tier_alone_is_not_warm() {
        let err = read_with(&[("OSS_WARM_ON_JOIN", "true")]).expect_err("refused");
        assert!(err.contains("OSS_PAGE_CACHE_DIR"), "{err}");
        assert_eq!(read_with(&[]), Ok(None));
    }
}
