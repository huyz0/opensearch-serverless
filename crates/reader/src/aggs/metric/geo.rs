//! `geo_centroid` and `geo_bounds`: the shard-level values built from the
//! engine's per-bucket geo state, their reduce across shards
//! (`InternalGeoCentroid.reduce`, `InternalGeoBounds.reduce`) and their
//! response bodies.

use std::io::{self, Write};

use lucene_aggs::geo_aggs::GeoState;

use crate::aggs::metric::double::write_java_double;
use crate::aggs::partial::Value;

/// `geo_centroid`'s shard result: the sums over the count
/// (`GeoCentroidAggregator.buildAggregation`).
pub(crate) fn centroid_of(state: &GeoState) -> Value {
    if state.count == 0 {
        return Value::GeoCentroid {
            count: 0,
            lat: 0.0,
            lon: 0.0,
        };
    }
    let count = state.count as f64;
    Value::GeoCentroid {
        count: state.count,
        lat: state.lat_sum / count,
        lon: state.lon_sum / count,
    }
}

/// `geo_bounds`' shard result: the six extremes, as the aggregator keeps them.
pub(crate) fn bounds_of(state: &GeoState, wrap: bool) -> Value {
    Value::GeoBounds {
        top: state.top,
        bottom: state.bottom,
        pos_left: state.pos_left,
        pos_right: state.pos_right,
        neg_left: state.neg_left,
        neg_right: state.neg_right,
        wrap,
    }
}

/// `InternalGeoCentroid.reduce`: each shard's centroid weighted by its
/// count, summed in shard order. `None` unless every part is a centroid.
pub(crate) fn reduce_centroid(parts: &[Value]) -> Option<Value> {
    let mut sums: Option<(f64, f64)> = None;
    let mut total = 0u64;
    for part in parts {
        let Value::GeoCentroid { count, lat, lon } = part else {
            return None;
        };
        if *count == 0 {
            continue;
        }
        total = total.saturating_add(*count);
        let weight = *count as f64;
        sums = Some(match sums {
            None => (weight * lat, weight * lon),
            Some((la, lo)) => (la + weight * lat, lo + weight * lon),
        });
    }
    Some(match sums {
        None => Value::GeoCentroid {
            count: 0,
            lat: 0.0,
            lon: 0.0,
        },
        Some((lat, lon)) => Value::GeoCentroid {
            count: total,
            lat: lat / total as f64,
            lon: lon / total as f64,
        },
    })
}

/// `InternalGeoBounds.reduce`: the extremes of the shards' extremes. The
/// wrap flag is the first part's, as the reduce keeps `this`'s. `None`
/// unless every part is a `geo_bounds`.
pub(crate) fn reduce_bounds(parts: &[Value]) -> Option<Value> {
    let mut out = Value::GeoBounds {
        top: f64::NEG_INFINITY,
        bottom: f64::INFINITY,
        pos_left: f64::INFINITY,
        pos_right: f64::NEG_INFINITY,
        neg_left: f64::INFINITY,
        neg_right: f64::NEG_INFINITY,
        wrap: true,
    };
    for (i, part) in parts.iter().enumerate() {
        let (
            Value::GeoBounds {
                top,
                bottom,
                pos_left,
                pos_right,
                neg_left,
                neg_right,
                wrap,
            },
            Value::GeoBounds {
                top: t,
                bottom: b,
                pos_left: pl,
                pos_right: pr,
                neg_left: nl,
                neg_right: nr,
                wrap: w,
            },
        ) = (&mut out, part)
        else {
            return None;
        };
        if i == 0 {
            *wrap = *w;
        }
        if *t > *top {
            *top = *t;
        }
        if *b < *bottom {
            *bottom = *b;
        }
        if *pl < *pos_left {
            *pos_left = *pl;
        }
        if *pr > *pos_right {
            *pos_right = *pr;
        }
        if *nl < *neg_left {
            *neg_left = *nl;
        }
        if *nr > *neg_right {
            *neg_right = *nr;
        }
    }
    Some(out)
}

/// `geo_centroid`'s `doXContentBody`: `location` when there is a centroid,
/// then `count`.
pub(crate) fn write_centroid(
    out: &mut dyn Write,
    count: u64,
    lat: f64,
    lon: f64,
) -> io::Result<()> {
    if count > 0 {
        out.write_all(b"\"location\":{\"lat\":")?;
        write_java_double(out, lat)?;
        out.write_all(b",\"lon\":")?;
        write_java_double(out, lon)?;
        out.write_all(b"},")?;
    }
    write!(out, "\"count\":{count}")
}

/// `InternalGeoBounds.resolveGeoBoundingBox`: the top-left and
/// bottom-right corners as `(lat, lon)`, or `None` when no point was seen.
#[allow(clippy::too_many_arguments)]
fn resolve(
    top: f64,
    bottom: f64,
    pos_left: f64,
    pos_right: f64,
    neg_left: f64,
    neg_right: f64,
    wrap: bool,
) -> Option<((f64, f64), (f64, f64))> {
    if top.is_infinite() {
        None
    } else if pos_left.is_infinite() {
        Some(((top, neg_left), (bottom, neg_right)))
    } else if neg_left.is_infinite() {
        Some(((top, pos_left), (bottom, pos_right)))
    } else if wrap {
        let unwrapped = pos_right - neg_left;
        let wrapped = (180.0 - pos_left) - (-180.0 - neg_right);
        if unwrapped <= wrapped {
            Some(((top, neg_left), (bottom, pos_right)))
        } else {
            Some(((top, pos_left), (bottom, neg_right)))
        }
    } else {
        Some(((top, neg_left), (bottom, pos_right)))
    }
}

/// `geo_bounds`' `doXContentBody`: `bounds` with its two corners, nothing
/// for no point.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_bounds(
    out: &mut dyn Write,
    top: f64,
    bottom: f64,
    pos_left: f64,
    pos_right: f64,
    neg_left: f64,
    neg_right: f64,
    wrap: bool,
) -> io::Result<()> {
    let Some((left, right)) = resolve(top, bottom, pos_left, pos_right, neg_left, neg_right, wrap)
    else {
        return Ok(());
    };
    out.write_all(b"\"bounds\":{\"top_left\":{\"lat\":")?;
    write_java_double(out, left.0)?;
    out.write_all(b",\"lon\":")?;
    write_java_double(out, left.1)?;
    out.write_all(b"},\"bottom_right\":{\"lat\":")?;
    write_java_double(out, right.0)?;
    out.write_all(b",\"lon\":")?;
    write_java_double(out, right.1)?;
    out.write_all(b"}}")
}

#[cfg(test)]
#[path = "geo_tests.rs"]
mod tests;
