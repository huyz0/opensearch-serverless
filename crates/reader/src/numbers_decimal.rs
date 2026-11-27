//! Query values for `float` and `double` fields, read as `NumberFieldMapper`
//! reads them.
//!
//! Ported from `NumberFieldMapper.NumberType.FLOAT` and `DOUBLE`'s `parse`,
//! `termQuery`, `termsQuery` and `rangeQuery`, and `DoublePoint` and
//! `FloatPoint`'s `nextUp`, `nextDown` and sortable encodings, at OpenSearch
//! 3.10.0. A value is parsed as `Double.parseDouble` parses it (a float then
//! narrowed), and a value that is not finite is refused as the mapper refuses it.

use lucene_search::Result as SearchResult;
use lucene_search::extended_query::{PointInSetQuery, PointRangeQuery, int_bytes, long_bytes};
use lucene_util::half_float::{half_float_to_sortable_short, sortable_short_to_half_float};
use lucene_util::numeric_utils::{double_to_sortable_long, float_to_sortable_int};

use crate::dsl::Value;
use crate::failure::Failure;
use crate::numbers::{Bounds, java_double};

fn not_finite(ty: &str, shown: &str) -> Failure {
    Failure::plain(
        "illegal_argument_exception",
        // ALLOC: error path; one message.
        format!("[{ty}] supports only finite values, but got [{shown}]"),
    )
}

/// `DOUBLE.parse(value, false)`: a finite double.
pub fn parse_double(value: &Value) -> Result<f64, Failure> {
    let d = java_double(value.text())?;
    if d.is_finite() {
        Ok(d)
    } else {
        Err(not_finite("double", &crate::template::java_double(d)))
    }
}

/// `FLOAT.parse(value, false)`: a finite float.
pub fn parse_float(value: &Value) -> Result<f32, Failure> {
    let d = java_double(value.text())?;
    // Java's `floatValue` rounds to the nearest float, as `as` does.
    #[allow(clippy::cast_possible_truncation)]
    let f = d as f32;
    if f.is_finite() {
        return Ok(f);
    }
    // ALLOC: error path; one float's text.
    let mut shown = Vec::new();
    // Writing to a vector cannot fail.
    let _ = crate::response::write_java_float(&mut shown, f);
    Err(not_finite("float", &String::from_utf8_lossy(&shown)))
}

/// `HALF_FLOAT.parse(value, false)`: a float that is finite as a half.
pub fn parse_half(value: &Value) -> Result<f32, Failure> {
    let d = java_double(value.text())?;
    // Java's `floatValue` rounds to the nearest float, as `as` does.
    #[allow(clippy::cast_possible_truncation)]
    let f = d as f32;
    if sortable_short_to_half_float(half_float_to_sortable_short(f)).is_finite() {
        return Ok(f);
    }
    // ALLOC: error path; one float's text.
    let mut shown = Vec::new();
    // Writing to a vector cannot fail.
    let _ = crate::response::write_java_float(&mut shown, f);
    Err(not_finite("half_float", &String::from_utf8_lossy(&shown)))
}

/// `DoublePoint.nextUp`: `-0.0` steps to `+0.0`.
fn double_up(d: f64) -> f64 {
    if d.to_bits() == (1 << 63) {
        0.0
    } else {
        d.next_up()
    }
}

/// `DoublePoint.nextDown`: `+0.0` steps to `-0.0`.
fn double_down(d: f64) -> f64 {
    if d.to_bits() == 0 {
        -0.0
    } else {
        d.next_down()
    }
}

/// `FloatPoint.nextUp`.
fn float_up(f: f32) -> f32 {
    if f.to_bits() == (1 << 31) {
        0.0
    } else {
        f.next_up()
    }
}

/// `FloatPoint.nextDown`.
fn float_down(f: f32) -> f32 {
    if f.to_bits() == 0 {
        -0.0
    } else {
        f.next_down()
    }
}

/// `DOUBLE.rangeQuery`'s bounds: an excluded bound steps one double in.
pub fn double_range(
    from: Option<&Value>,
    to: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> Result<Bounds<f64>, Failure> {
    let mut l = f64::NEG_INFINITY;
    let mut u = f64::INFINITY;
    if let Some(v) = from {
        l = parse_double(v)?;
        if !include_lower {
            l = double_up(l);
        }
    }
    if let Some(v) = to {
        u = parse_double(v)?;
        if !include_upper {
            u = double_down(u);
        }
    }
    Ok((double_to_sortable_long(l) <= double_to_sortable_long(u)).then_some((l, u)))
}

/// `FLOAT.rangeQuery`'s bounds, as [`double_range`]'s.
pub fn float_range(
    from: Option<&Value>,
    to: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> Result<Bounds<f32>, Failure> {
    let mut l = f32::NEG_INFINITY;
    let mut u = f32::INFINITY;
    if let Some(v) = from {
        l = parse_float(v)?;
        if !include_lower {
            l = float_up(l);
        }
    }
    if let Some(v) = to {
        u = parse_float(v)?;
        if !include_upper {
            u = float_down(u);
        }
    }
    Ok((float_to_sortable_int(l) <= float_to_sortable_int(u)).then_some((l, u)))
}

/// `HALF_FLOAT.rangeQuery`'s bounds as sortable shorts: a bound is rounded to
/// a half, and an excluded one steps one half in (`HalfFloatPoint.nextUp` and
/// `nextDown`, which are one step of the sortable short, `-0.0` and `+0.0`
/// included).
pub fn half_range(
    from: Option<&Value>,
    to: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> Result<Bounds<i16>, Failure> {
    let mut l = half_float_to_sortable_short(f32::NEG_INFINITY);
    let mut u = half_float_to_sortable_short(f32::INFINITY);
    if let Some(v) = from {
        l = half_float_to_sortable_short(parse_half(v)?);
        if !include_lower {
            l = l.saturating_add(1);
        }
    }
    if let Some(v) = to {
        u = half_float_to_sortable_short(parse_half(v)?);
        if !include_upper {
            u = u.saturating_sub(1);
        }
    }
    Ok((l <= u).then_some((l, u)))
}

/// `HalfFloatPoint.newRangeQuery` over sortable shorts.
pub fn half_points(field: &str, lower: i16, upper: i16) -> SearchResult<PointRangeQuery> {
    PointRangeQuery::new(field, 1, short_bytes(lower), short_bytes(upper))
}

/// `HalfFloatPoint.newSetQuery`.
pub fn half_set(field: &str, values: &[f32]) -> SearchResult<PointInSetQuery> {
    PointInSetQuery::new(
        field,
        1,
        2,
        // ALLOC: one packed point a value, bounded by the body's recording.
        values
            .iter()
            .map(|&v| short_bytes(half_float_to_sortable_short(v))),
    )
}

/// `NumericUtils.shortToSortableBytes`.
fn short_bytes(v: i16) -> Vec<u8> {
    // ALLOC: one packed point of 2 bytes.
    (v ^ i16::MIN).to_be_bytes().to_vec()
}

/// `DoublePoint.newRangeQuery`.
pub fn double_points(field: &str, lower: f64, upper: f64) -> SearchResult<PointRangeQuery> {
    // ALLOC: the packed bounds of one point query.
    let bytes = |v: f64| long_bytes(double_to_sortable_long(v)).to_vec();
    PointRangeQuery::new(field, 1, bytes(lower), bytes(upper))
}

/// `FloatPoint.newRangeQuery`.
pub fn float_points(field: &str, lower: f32, upper: f32) -> SearchResult<PointRangeQuery> {
    // ALLOC: the packed bounds of one point query.
    let bytes = |v: f32| int_bytes(float_to_sortable_int(v)).to_vec();
    PointRangeQuery::new(field, 1, bytes(lower), bytes(upper))
}

/// `DoublePoint.newSetQuery`.
pub fn double_set(field: &str, values: &[f64]) -> SearchResult<PointInSetQuery> {
    PointInSetQuery::new(
        field,
        1,
        8,
        // ALLOC: one packed point a value, bounded by the body's recording.
        values
            .iter()
            .map(|&v| long_bytes(double_to_sortable_long(v)).to_vec()),
    )
}

/// `FloatPoint.newSetQuery`.
pub fn float_set(field: &str, values: &[f32]) -> SearchResult<PointInSetQuery> {
    PointInSetQuery::new(
        field,
        1,
        4,
        // ALLOC: one packed point a value, bounded by the body's recording.
        values
            .iter()
            .map(|&v| int_bytes(float_to_sortable_int(v)).to_vec()),
    )
}

#[cfg(test)]
#[path = "numbers_decimal_tests.rs"]
mod tests;
