//! `typed_keys`: an aggregation's key in the response written as `type#name`.
//!
//! Ported from `InternalAggregation.getWriteableName` and `Aggregations.toXContent`
//! (server/src/main/java/org/opensearch/search/aggregations/) at OpenSearch 3.10.0. A scope
//! over one write turns the prefix on for the aggregations and for the sub-aggregations of every
//! bucket under them; it is thread-local because a write runs to its end on one thread and the
//! bucket writers have no other way to reach it without a parameter on each.

use std::cell::Cell;
use std::io::{self, Write};

use oss_rest::write_json_str;

use crate::aggs::partial::Value;

thread_local! {
    /// Whether the write open on this thread types its keys.
    static ON: Cell<bool> = const { Cell::new(false) };
}

/// Runs `write` with typed keys on or off, restoring what was there before.
pub fn scoped<R>(on: bool, write: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            ON.with(|cell| cell.set(self.0));
        }
    }
    let _restore = Restore(ON.with(|cell| cell.replace(on)));
    write()
}

/// The type name `typed_keys` puts before an aggregation's name, for the aggregations served.
fn type_name(value: &Value) -> Option<&'static str> {
    Some(match value {
        Value::Min(_) => "min",
        Value::Max(_) => "max",
        Value::Sum(_) => "sum",
        Value::Avg { .. } => "avg",
        Value::Stats { .. } => "stats",
        Value::ValueCount(_) => "value_count",
        Value::Cardinality(_) => "cardinality",
        Value::Percentiles { ranks: false, .. } => "tdigest_percentiles",
        Value::Percentiles { ranks: true, .. } => "tdigest_percentile_ranks",
        Value::TopHits(_) | Value::TopHitsParts(_) | Value::Rendered(_) => "top_hits",
        Value::Terms(_) => "lterms",
        Value::KeywordTerms(_) => "sterms",
        Value::Histogram(_) => "histogram",
        Value::DateHistogram(_) => "date_histogram",
        Value::Range(_) => "range",
        Value::Filters(result) => result.kind.type_name(),
        Value::Composite(_) => "composite",
    })
}

/// An aggregation's key: its name, or `type#name` inside a typed scope.
pub fn write_key(out: &mut dyn Write, name: &str, value: &Value) -> io::Result<()> {
    match type_name(value).filter(|_| ON.with(Cell::get)) {
        // ALLOC: one key an aggregation, bounded by the request's.
        Some(ty) => write_json_str(out, &format!("{ty}#{name}")),
        None => write_json_str(out, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &Value) -> String {
        let mut out = Vec::new();
        write_key(&mut out, "n", value).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_scope_prefixes_the_keys_inside_it_and_restores_what_was_before() {
        let min = Value::Min(0.0);
        assert_eq!(key(&min), r#""n""#);
        scoped(true, || {
            assert_eq!(key(&min), r#""min#n""#);
            // A scope inside one of another setting returns to the outer one.
            scoped(false, || assert_eq!(key(&min), r#""n""#));
            assert_eq!(key(&min), r#""min#n""#);
        });
        assert_eq!(key(&min), r#""n""#);
    }

    #[test]
    fn a_scope_restores_the_setting_when_the_write_panics() {
        let caught = std::panic::catch_unwind(|| scoped(true, || panic!("a failed write")));
        assert!(caught.is_err());
        assert_eq!(key(&Value::Sum(0.0)), r#""n""#);
    }

    #[test]
    fn every_value_has_the_type_its_java_class_writes() {
        use crate::aggs::bucket::filters::{FilterKind, FiltersResult};
        let single = Value::Filters(Box::new(FiltersResult {
            kind: FilterKind::Filter,
            single: true,
            ..FiltersResult::default()
        }));
        let kinds = [
            (FilterKind::Missing, r#""missing#n""#),
            (FilterKind::Nested, r#""nested#n""#),
            (FilterKind::ReverseNested, r#""reverse_nested#n""#),
        ];
        let several = Value::Filters(Box::default());
        scoped(true, || {
            assert_eq!(key(&single), r#""filter#n""#);
            for (kind, written) in kinds {
                let value = Value::Filters(Box::new(FiltersResult {
                    kind,
                    single: true,
                    ..FiltersResult::default()
                }));
                assert_eq!(key(&value), written);
            }
            assert_eq!(key(&several), r#""filters#n""#);
            assert_eq!(key(&Value::ValueCount(0)), r#""value_count#n""#);
            assert_eq!(key(&Value::Rendered(Vec::new())), r#""top_hits#n""#);
        });
    }
}
