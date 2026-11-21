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
        Value::Histogram(r) => nested(&r.subs.lists),
        Value::DateHistogram(r) => nested(&r.subs.lists),
        Value::KeywordTerms(r) => nested(&r.subs),
        Value::Range(r) => nested(&r.subs),
        Value::Filters(r) => nested(&r.subs),
        _ => false,
    }
}
