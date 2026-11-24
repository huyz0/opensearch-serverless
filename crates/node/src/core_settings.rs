//! The core index settings a create accepts without the node reading them:
//! each is checked and stored in the descriptor, and has no effect on a
//! serverless node, because what it tunes does not exist here (a local
//! translog, replica allocation, slow logs, a shard's local cache choices).
//!
//! A setting that would change what the node does is not here: it stays
//! refused 501, because storing it and ignoring it would break the promise
//! the client made by setting it (a read-only block, a result window, an
//! analyzer, a mapping limit).

use oss_rest::RestError;

use crate::time_value::time_millis;

/// How a value is checked.
#[derive(Clone, Copy)]
enum Check {
    Bool,
    Int,
    /// A time value or `-1`.
    Time,
    Choice(&'static [&'static str]),
    /// Any non-empty text.
    Text,
}

/// Keys named in full.
const KEYS: &[(&str, Check)] = &[
    (
        "index.codec",
        Check::Choice(&[
            "default",
            "best_compression",
            "lz4",
            "zlib",
            "zstd",
            "zstd_no_dict",
        ]),
    ),
    (
        "index.translog.durability",
        Check::Choice(&["request", "async"]),
    ),
    ("index.translog.sync_interval", Check::Time),
    ("index.priority", Check::Int),
    ("index.unassigned.node_left.delayed_timeout", Check::Time),
    ("index.search.idle.after", Check::Time),
    ("index.gc_deletes", Check::Time),
    ("index.queries.cache.enabled", Check::Bool),
    ("index.requests.cache.enable", Check::Bool),
    (
        "index.shard.check_on_startup",
        Check::Choice(&["false", "true", "checksum"]),
    ),
    (
        "index.search.slowlog.level",
        Check::Choice(&["warn", "info", "debug", "trace"]),
    ),
    (
        "index.indexing.slowlog.level",
        Check::Choice(&["warn", "info", "debug", "trace"]),
    ),
];

/// Keys that start with a prefix: the allocation filters and the slow log
/// thresholds.
const PREFIXES: &[(&str, Check)] = &[
    ("index.routing.allocation.include.", Check::Text),
    ("index.routing.allocation.exclude.", Check::Text),
    ("index.routing.allocation.require.", Check::Text),
    ("index.search.slowlog.threshold.", Check::Time),
    ("index.indexing.slowlog.threshold.", Check::Time),
];

fn check_of(key: &str) -> Option<Check> {
    KEYS.iter()
        .find(|(k, _)| *k == key)
        .or_else(|| {
            PREFIXES
                .iter()
                .find(|(p, _)| key.len() > p.len() && key.starts_with(p))
        })
        .map(|(_, check)| *check)
}

/// Whether `key` is a core setting a create accepts and stores.
pub(crate) fn accepts(key: &str) -> bool {
    check_of(key).is_some()
}

/// Checks `value` of the accepted core setting `key`; `Ok(false)` for a key
/// that is not one.
pub(crate) fn check(key: &str, value: &str) -> Result<bool, RestError> {
    let Some(check) = check_of(key) else {
        return Ok(false);
    };
    let bad = |what: &str| {
        // ALLOC: error path; one message.
        RestError::illegal_argument(format!(
            "failed to parse value [{value}] for setting [{key}] as {what}"
        ))
    };
    match check {
        Check::Bool => {
            if value != "true" && value != "false" {
                return Err(bad("a boolean: only [true] or [false] are allowed"));
            }
        }
        Check::Int => {
            value.parse::<i32>().map_err(|_| bad("a whole number"))?;
        }
        Check::Time => {
            if value != "-1" {
                time_millis(key, value)?;
            }
        }
        Check::Choice(allowed) => {
            if !allowed.contains(&value) {
                return Err(RestError::illegal_argument(format!(
                    // ALLOC: error path; one message.
                    "unknown value for [{key}], must be one of [{}] but was [{value}]",
                    allowed.join(", ")
                )));
            }
        }
        Check::Text => {
            if value.is_empty() {
                return Err(bad("text: it must not be empty"));
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_accepted_by_name_or_under_a_prefix_but_not_the_prefix_alone() {
        assert!(accepts("index.codec"));
        assert!(accepts("index.routing.allocation.include._zone"));
        assert!(accepts("index.search.slowlog.threshold.query.warn"));
        assert!(!accepts("index.routing.allocation.include."));
        assert!(!accepts("index.blocks.read_only"));
        assert!(!accepts("index.max_result_window"));
    }

    #[test]
    fn each_check_takes_its_values_and_refuses_the_rest() {
        let good = [
            ("index.codec", "best_compression"),
            ("index.priority", "-3"),
            ("index.queries.cache.enabled", "false"),
            ("index.search.slowlog.threshold.fetch.warn", "-1"),
            ("index.search.slowlog.threshold.fetch.info", "10s"),
            ("index.routing.allocation.require.box_type", "hot"),
            ("index.translog.durability", "async"),
        ];
        for (key, value) in good {
            assert_eq!(check(key, value).ok(), Some(true), "{key}={value}");
        }
        let bad = [
            ("index.codec", "gzip"),
            ("index.priority", "high"),
            ("index.queries.cache.enabled", "yes"),
            ("index.gc_deletes", "soon"),
            ("index.routing.allocation.include._zone", ""),
        ];
        for (key, value) in bad {
            assert!(check(key, value).is_err(), "{key}={value}");
        }
        assert_eq!(check("index.blocks.write", "true").ok(), Some(false));
    }
}
