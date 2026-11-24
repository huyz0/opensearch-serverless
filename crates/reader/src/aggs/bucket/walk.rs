//! A walk over the sub-aggregation values of a result's buckets, at any
//! depth, by the name of the sub-aggregation each value belongs to.

use crate::aggs::partial::{Format, Value};

/// Calls `f` with the name and a mutable reference of each sub-aggregation
/// value among the buckets of `value`, and then walks into the values that
/// hold buckets of their own. The first error ends the walk.
///
/// # Errors
///
/// The first error `f` returns.
pub fn walk_subs_mut<E>(
    value: &mut Value,
    f: &mut dyn FnMut(&str, &mut Value) -> Result<(), E>,
) -> Result<(), E> {
    match value {
        Value::Terms(r) => visit(&r.subs.meta, &mut r.subs.lists, f),
        Value::Composite(r) => visit(&r.subs.meta, &mut r.subs.lists, f),
        Value::Histogram(r) => visit(&r.subs.meta, &mut r.subs.lists, f),
        Value::DateHistogram(r) => visit(&r.subs.meta, &mut r.subs.lists, f),
        Value::KeywordTerms(r) => visit(&r.sub_meta, &mut r.subs, f),
        Value::Range(r) => visit(&r.sub_meta, &mut r.subs, f),
        Value::Filters(r) => visit(&r.sub_meta, &mut r.subs, f),
        _ => Ok(()),
    }
}

fn visit<E>(
    meta: &[(String, Format)],
    lists: &mut [Vec<Value>],
    f: &mut dyn FnMut(&str, &mut Value) -> Result<(), E>,
) -> Result<(), E> {
    for list in lists {
        for ((name, _), value) in meta.iter().zip(list.iter_mut()) {
            f(name, value)?;
            walk_subs_mut(value, f)?;
        }
    }
    Ok(())
}

/// Whether any bucket of `value` holds a sub-aggregation value `f` accepts,
/// at any depth.
pub fn any_sub(value: &Value, f: &dyn Fn(&Value) -> bool) -> bool {
    let nested = |lists: &[Vec<Value>]| lists.iter().flatten().any(|v| f(v) || any_sub(v, f));
    match value {
        Value::Terms(r) => nested(&r.subs.lists),
        Value::Composite(r) => nested(&r.subs.lists),
        Value::Histogram(r) => nested(&r.subs.lists),
        Value::DateHistogram(r) => nested(&r.subs.lists),
        Value::KeywordTerms(r) => nested(&r.subs),
        Value::Range(r) => nested(&r.subs),
        Value::Filters(r) => nested(&r.subs),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggs::bucket::keyword_terms::KeywordTermsResult;
    use crate::aggs::bucket::sub_values::BucketSubs;
    use crate::aggs::bucket::{
        date_histogram::DateHistogramResult, filters::FiltersResult, histogram::HistogramResult,
        range::RangeResult, terms::TermsResult,
    };

    fn leaf(tag: u8) -> Value {
        Value::Rendered(vec![tag])
    }

    fn meta() -> Vec<(String, Format)> {
        vec![("sub".to_owned(), Format::Raw)]
    }

    fn subs(inner: Value) -> BucketSubs {
        BucketSubs {
            meta: meta(),
            empty: Vec::new(),
            lists: vec![vec![inner]],
        }
    }

    /// One of each bucket value, every one holding `inner` under the name `sub`.
    fn holders(inner: &Value) -> Vec<(&'static str, Value)> {
        let rows = || vec![vec![inner.clone()]];
        vec![
            (
                "terms",
                Value::Terms(Box::new(TermsResult {
                    subs: subs(inner.clone()),
                    ..TermsResult::default()
                })),
            ),
            (
                "histogram",
                Value::Histogram(Box::new(HistogramResult {
                    subs: subs(inner.clone()),
                    ..HistogramResult::default()
                })),
            ),
            (
                "date_histogram",
                Value::DateHistogram(Box::new(DateHistogramResult {
                    subs: subs(inner.clone()),
                    ..DateHistogramResult::default()
                })),
            ),
            (
                "keyword_terms",
                Value::KeywordTerms(Box::new(KeywordTermsResult {
                    spec: Default::default(),
                    other_doc_count: 0,
                    doc_count_error: 0,
                    buckets: Vec::new(),
                    bucket_errors: Vec::new(),
                    sub_meta: meta(),
                    subs: rows(),
                })),
            ),
            (
                "range",
                Value::Range(Box::new(RangeResult {
                    sub_meta: meta(),
                    subs: rows(),
                    ..RangeResult::default()
                })),
            ),
            (
                "filters",
                Value::Filters(Box::new(FiltersResult {
                    sub_meta: meta(),
                    subs: rows(),
                    ..FiltersResult::default()
                })),
            ),
        ]
    }

    #[test]
    fn walk_visits_and_edits_the_values_of_every_bucket_kind() {
        for (kind, mut value) in holders(&leaf(1)) {
            let mut seen = Vec::new();
            walk_subs_mut::<()>(&mut value, &mut |name, v| {
                seen.push(name.to_owned());
                *v = leaf(2);
                Ok(())
            })
            .unwrap();
            assert_eq!(seen, ["sub"], "{kind}");
            let mut after = Vec::new();
            walk_subs_mut::<()>(&mut value, &mut |_, v| {
                after.push(v.clone());
                Ok(())
            })
            .unwrap();
            assert_eq!(after, [leaf(2)], "{kind}");
        }
    }

    #[test]
    fn walk_goes_into_buckets_nested_in_buckets_and_stops_at_the_first_error() {
        for (kind, inner) in holders(&leaf(1)) {
            for (outer_kind, mut outer) in holders(&inner) {
                let mut seen = 0;
                walk_subs_mut::<()>(&mut outer, &mut |_, _| {
                    seen += 1;
                    Ok(())
                })
                .unwrap();
                assert_eq!(seen, 2, "{outer_kind} over {kind}");
                let mut calls = 0;
                let err = walk_subs_mut(&mut outer, &mut |_, _| {
                    calls += 1;
                    Err("stop")
                });
                assert_eq!((err, calls), (Err("stop"), 1), "{outer_kind} over {kind}");
            }
        }
        let mut plain = leaf(1);
        walk_subs_mut::<()>(&mut plain, &mut |_, _| panic!("a leaf has no buckets")).unwrap();
    }

    #[test]
    fn any_sub_finds_a_value_at_any_depth_in_every_bucket_kind() {
        let is_one = |v: &Value| *v == leaf(1);
        for (kind, value) in holders(&leaf(1)) {
            assert!(any_sub(&value, &is_one), "{kind}");
            assert!(!any_sub(&value, &|v| *v == leaf(9)), "{kind}");
            for (outer_kind, outer) in holders(&value) {
                assert!(any_sub(&outer, &is_one), "{outer_kind} over {kind}");
                // Only the inner bucket value is a bucket: `f` accepts it before the walk goes deeper.
                let is_bucket = |v: &Value| !matches!(v, Value::Rendered(_));
                assert!(any_sub(&outer, &is_bucket), "{outer_kind} over {kind}");
            }
        }
        assert!(!any_sub(&leaf(1), &|_| true));
    }
}
