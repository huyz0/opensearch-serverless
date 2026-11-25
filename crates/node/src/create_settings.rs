//! The index settings a create may carry: the ones the node reads from the
//! descriptor and the core ones it only stores ([`crate::core_settings`]),
//! checked, and a refusal naming any other key.

use oss_rest::{Exception, RestError};

use crate::time_value::time_millis;

/// `index.refresh_interval`: how long a write may wait to be published.
pub(crate) const REFRESH_INTERVAL: &str = "index.refresh_interval";
/// `index.routing_partition_size`: the shards one routing value spreads over.
pub(crate) const PARTITION_SIZE: &str = "index.routing_partition_size";
/// `index.number_of_routing_shards`: the shard count routing is computed over.
pub(crate) const ROUTING_SHARDS: &str = "index.number_of_routing_shards";

/// `index.default_pipeline`: the pipeline a write that names none runs.
pub(crate) const DEFAULT_PIPELINE: &str = "index.default_pipeline";
/// `index.final_pipeline`: the pipeline every write runs last.
pub(crate) const FINAL_PIPELINE: &str = "index.final_pipeline";

const READ: [&str; 5] = [
    REFRESH_INTERVAL,
    PARTITION_SIZE,
    ROUTING_SHARDS,
    DEFAULT_PIPELINE,
    FINAL_PIPELINE,
];

/// Whether `key` is a setting a create reads or stores.
pub(crate) fn is_known(key: &str) -> bool {
    READ.contains(&key) || crate::core_settings::accepts(key)
}

/// Whether `key` is a known setting that only a create may set: it is read
/// when a shard opens, or core declares it static.
pub(crate) fn is_static(key: &str) -> bool {
    matches!(
        key,
        PARTITION_SIZE
            | ROUTING_SHARDS
            | "index.codec"
            | "index.queries.cache.enabled"
            | "index.shard.check_on_startup"
    )
}

fn refuse(status: u16, kind: &'static str, reason: impl Into<String>) -> RestError {
    RestError::exception(status, Exception::new(kind, reason))
}

fn bad(reason: String) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

/// Refuses, naming every one, the keys of `settings` that are neither read
/// from the descriptor nor `own` (the bucket and cost settings).
pub(crate) fn refuse_others(
    settings: &[(String, Option<String>)],
    own: impl Fn(&str) -> bool,
) -> Result<(), RestError> {
    // ALLOC: the refused keys, bounded by the body.
    let mut others: Vec<&str> = settings
        .iter()
        .map(|(key, _)| key.as_str())
        .filter(|key| !READ.contains(key) && !crate::core_settings::accepts(key) && !own(key))
        .collect();
    if others.is_empty() {
        return Ok(());
    }
    others.sort_unstable();
    Err(refuse(
        501,
        "unsupported_index_option",
        // ALLOC: error path; one message.
        format!(
            "index setting{} [{}] not supported by this node at create yet: create the index, then update its settings",
            if others.len() > 1 { "s" } else { "" },
            others.join(", ")
        ),
    ))
}

/// The checked values of the settings the node reads, for an index of
/// `shards`; a null is dropped. The pipeline settings are accepted when
/// `pipelines` says the node runs them. Sorted by key.
pub(crate) fn checked(
    settings: &[(String, Option<String>)],
    shards: u32,
    pipelines: bool,
) -> Result<Vec<(String, String)>, RestError> {
    // ALLOC: at most three entries.
    let mut out: Vec<(String, String)> = Vec::new();
    for (key, value) in settings {
        let Some(value) = value.as_deref().map(str::trim) else {
            continue;
        };
        let key = key.as_str();
        match key {
            REFRESH_INTERVAL => {
                if value != "-1" {
                    time_millis(key, value)?;
                }
            }
            PARTITION_SIZE => {
                let size = whole(key, value)?;
                let fits = if shards == 1 {
                    size == 1
                } else {
                    (1..shards).contains(&size)
                };
                if !fits {
                    return Err(bad(format!(
                        "{key} [{size}] must be at least 1 and less than the number of shards [{shards}] (1 for a one-shard index)"
                    )));
                }
            }
            ROUTING_SHARDS => {
                let routing = whole(key, value)?;
                if routing < shards || routing.checked_rem(shards) != Some(0) {
                    return Err(bad(format!(
                        "{key} [{routing}] must be a multiple of the number of shards [{shards}]"
                    )));
                }
            }
            DEFAULT_PIPELINE | FINAL_PIPELINE => {
                if !pipelines {
                    return Err(refuse(
                        501,
                        "unsupported_index_option",
                        // ALLOC: error path; one message.
                        format!("{key} is not supported: this node does not run ingest pipelines"),
                    ));
                }
                if value.is_empty() {
                    return Err(bad(format!("{key} must not be empty")));
                }
            }
            _ => {
                if !crate::core_settings::check(key, value)? {
                    continue;
                }
            }
        }
        out.push((key.to_owned(), value.to_owned()));
    }
    out.sort();
    Ok(out)
}

fn whole(key: &str, value: &str) -> Result<u32, RestError> {
    value.parse::<u32>().map_err(|_| {
        bad(format!(
            "failed to parse value [{value}] for setting [{key}] as a whole number"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn given(pairs: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), Some((*v).to_owned())))
            .collect()
    }

    #[test]
    fn the_three_are_kept_sorted_and_a_null_is_dropped() {
        let mut set = given(&[(ROUTING_SHARDS, "8"), (REFRESH_INTERVAL, "5s")]);
        set.push((PARTITION_SIZE.to_owned(), None));
        let kept = checked(&set, 4, false).expect("valid");
        assert_eq!(
            kept,
            vec![
                (ROUTING_SHARDS.to_owned(), "8".to_owned()),
                (REFRESH_INTERVAL.to_owned(), "5s".to_owned()),
            ]
        );
    }

    #[test]
    fn a_refresh_interval_is_a_time_value_or_minus_one() {
        for good in ["1s", "500ms", "0", "-1"] {
            assert!(
                checked(&given(&[(REFRESH_INTERVAL, good)]), 1, false).is_ok(),
                "{good}"
            );
        }
        for bad in ["fast", "1", "-2", "1.5s", ""] {
            assert!(
                checked(&given(&[(REFRESH_INTERVAL, bad)]), 1, false).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_partition_size_is_below_the_shard_count_or_one_for_one_shard() {
        let size = |n: &str, shards| checked(&given(&[(PARTITION_SIZE, n)]), shards, false).is_ok();
        assert!(size("1", 1) && !size("2", 1) && !size("0", 1));
        assert!(size("1", 4) && size("3", 4) && !size("4", 4) && !size("0", 4));
        assert!(!size("x", 4));
    }

    #[test]
    fn the_routing_shards_are_a_multiple_of_the_shard_count() {
        let routing =
            |n: &str, shards| checked(&given(&[(ROUTING_SHARDS, n)]), shards, false).is_ok();
        assert!(routing("4", 4) && routing("8", 4) && routing("1024", 4));
        assert!(!routing("2", 4) && !routing("6", 4) && !routing("0", 4) && !routing("x", 4));
    }

    #[test]
    fn other_keys_are_refused_together_sorted_and_own_keys_pass() {
        let set = given(&[
            ("index.blocks.write", "x"),
            ("index.b", "1"),
            ("own", "1"),
            (REFRESH_INTERVAL, "1s"),
        ]);
        let err = refuse_others(&set, |k| k == "own").expect_err("refused");
        assert_eq!(err.status(), 501);
        assert!(
            err.as_exception()
                .expect("a body")
                .reason()
                .contains("settings [index.b, index.blocks.write]"),
            "{err:?}"
        );
        assert!(refuse_others(&given(&[("own", "1")]), |k| k == "own").is_ok());
    }
}
