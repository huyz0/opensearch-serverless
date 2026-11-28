//! The `geo_bounding_box` query: `GeoBoundingBoxQueryBuilder.fromXContent`
//! and `doToQuery`, with `GeoBoundingBox.parseBoundingBox`
//! (server/src/main/java/org/opensearch/) at OpenSearch 3.10.0, over a
//! `geo_point` field as `VectorGeoPointShapeQueryProcessor` builds it: a
//! rectangle intersected, which is `LatLonPoint.newGeometryQuery`.

use std::sync::Arc;

use lucene_search::Clause;
use lucene_search::document::geo::{QueryRelation, lat_lon_point};
use lucene_search::extended_query::{DocumentClause, ExtendedQuery};
use lucene_util::geo::{LatLonGeometry, Rectangle};
use oss_doc::document::geo as point;

use super::geo_distance::{
    Validation, double_value, parse_array, shard_failure, validation_failure, validation_method,
};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, float_value, name_value};
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, match_none};
use crate::mapping::FieldType;
use oss_doc::mapping::json::java_double;

/// A parsed `geo_bounding_box` query.
#[derive(Clone, Debug, PartialEq)]
pub struct GeoBoundingBox {
    /// The field.
    pub field: String,
    /// The top latitude.
    pub top: f64,
    /// The left longitude.
    pub left: f64,
    /// The bottom latitude.
    pub bottom: f64,
    /// The right longitude.
    pub right: f64,
    /// `validation_method`.
    pub validation: Validation,
    /// `ignore_unmapped`.
    pub ignore_unmapped: bool,
    /// The boost.
    pub boost: f32,
}

/// `GeoUtils.EffectivePoint`: which corner of a geohash cell a corner
/// field takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

const INVALID_FIELDS: &str = "field must be either [lon|lat], [type|coordinates], or [geohash]";

/// `GeoPoint.parseGeoHash`: the corner of the geohash cell `text` names. The
/// cell is `ceil(5n/2)` longitude and `floor(5n/2)` latitude bits wide, each
/// at most the 26 bits the morton hash keeps.
fn geohash_corner(text: &str, corner: Corner) -> Result<(f64, f64), DslError> {
    let (lat, lon) = point::geohash(text).map_err(|e| parse_failure_text(e.reason))?;
    if corner == Corner::BottomLeft {
        return Ok((lat, lon));
    }
    let bits = u32::try_from(text.chars().count().min(12).saturating_mul(5)).unwrap_or(60);
    let lon_bits = bits.div_ceil(2).min(26);
    let lat_bits = (bits / 2).min(26);
    let dlon = 360.0 / f64::from(1u32 << lon_bits);
    let dlat = 180.0 / f64::from(1u32 << lat_bits);
    let top = (lat + dlat).min(90.0);
    let right = (lon + dlon).min(180.0);
    Ok(match corner {
        Corner::TopLeft => (top, lon),
        Corner::TopRight => (top, right),
        Corner::BottomRight => (lat, right),
        Corner::BottomLeft => (lat, lon),
    })
}

fn parse_failure_text(reason: impl Into<String>) -> DslError {
    DslError::Failure(Failure::plain("opensearch_parse_exception", reason))
}

/// `GeoPoint.resetFromString` for a corner.
fn string_corner(text: &str, corner: Corner) -> Result<(f64, f64), DslError> {
    if !text.to_lowercase().contains("point") && !text.contains(',') {
        return geohash_corner(text, corner);
    }
    point::string_point(text, false).map_err(|e| parse_failure_text(e.reason))
}

/// `GeoUtils.parseGeoPoint(parser, point, false, corner)`: the parser is on
/// the value's first token.
fn corner_point(p: &mut Parser<'_>, corner: Corner) -> Result<(f64, f64), DslError> {
    match p.current_token() {
        Some(Kind::StartArray) => parse_array(p),
        Some(Kind::String) => string_corner(p.text(), corner),
        Some(Kind::StartObject) => corner_object(p, corner),
        _ => Err(parse_failure_text("geo_point expected")),
    }
}

fn not_allowed(token: Option<Kind>) -> DslError {
    // ALLOC: one per request; bounded by the body's recording.
    parse_failure_text(format!(
        "token [{}] not allowed",
        token.map_or("null", Kind::name)
    ))
}

/// `parseGeoPointObject`: a `lat`/`lon`, `geohash` or GeoJSON point object.
fn corner_object(p: &mut Parser<'_>, corner: Corner) -> Result<(f64, f64), DslError> {
    if p.next_token() != Some(Kind::FieldName) {
        return Err(not_allowed(p.current_token()));
    }
    let found = match p.text() {
        "lat" | "lon" => basic_fields(p)?,
        "geohash" => {
            if p.next_token() != Some(Kind::String) {
                return Err(parse_failure_text("geohash must be a string"));
            }
            geohash_corner(p.text(), corner)?
        }
        "type" | "coordinates" => geojson_fields(p)?,
        _ => return Err(parse_failure_text(INVALID_FIELDS)),
    };
    if p.next_token() != Some(Kind::EndObject) {
        return Err(parse_failure_text(INVALID_FIELDS));
    }
    Ok(found)
}

/// `parseGeoPointObjectBasicFields`.
fn basic_fields(p: &mut Parser<'_>) -> Result<(f64, f64), DslError> {
    let (mut lat, mut lon) = (None, None);
    for i in 0..2 {
        if i != 0 {
            p.next_token();
        }
        if p.current_token() != Some(Kind::FieldName) {
            break;
        }
        let name = p.text();
        if name != "lat" && name != "lon" {
            return Err(parse_failure_text(INVALID_FIELDS));
        }
        let is_lat = name == "lat";
        let value = match p.next_token() {
            Some(Kind::Number | Kind::String) => double_value(p)
                .map_err(|_| parse_failure_text("[lon] and [lat] must be valid double values"))?,
            _ => {
                // ALLOC: one per request; bounded by the body's recording.
                return Err(parse_failure_text(format!(
                    "{} must be a number",
                    if is_lat { "lat" } else { "lon" }
                )));
            }
        };
        if is_lat {
            lat = Some(value);
        } else {
            lon = Some(value);
        }
    }
    let lon = lon.ok_or_else(|| parse_failure_text("field [lon] missing"))?;
    let lat = lat.ok_or_else(|| parse_failure_text("field [lat] missing"))?;
    Ok((lat, lon))
}

/// `parseGeoJsonFields`: `type` `Point` and `coordinates`, in either order.
fn geojson_fields(p: &mut Parser<'_>) -> Result<(f64, f64), DslError> {
    let (mut has_type, mut found) = (false, None);
    for i in 0..2 {
        if i != 0 {
            p.next_token();
        }
        if p.current_token() != Some(Kind::FieldName) {
            if !has_type {
                return Err(parse_failure_text("field [type] missing"));
            }
            return Err(parse_failure_text("field [coordinates] missing"));
        }
        match p.text() {
            "type" => {
                if p.next_token() != Some(Kind::String) {
                    return Err(parse_failure_text("type must be a string"));
                }
                if !p.text().eq_ignore_ascii_case("point") {
                    return Err(parse_failure_text("type must be Point"));
                }
                has_type = true;
            }
            "coordinates" => {
                if p.next_token() != Some(Kind::StartArray) {
                    return Err(parse_failure_text("coordinates must be an array"));
                }
                found = Some(parse_array(p)?);
            }
            _ => return Err(parse_failure_text(INVALID_FIELDS)),
        }
    }
    found.ok_or_else(|| parse_failure_text("field [coordinates] missing"))
}

/// The four numbers of WKT `BBOX (minLon, maxLon, maxLat, minLat)`.
fn wkt_envelope(text: &str) -> Option<[f64; 4]> {
    let rest = text.trim();
    let rest = rest
        .get(..4)
        .filter(|h| h.eq_ignore_ascii_case("bbox"))
        .and_then(|_| rest.get(4..))?
        .trim();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    let mut numbers = inner
        .split(',')
        .map(|n| oss_doc::mapping::java_parse_double(n.trim()));
    let found = [
        numbers.next()??,
        numbers.next()??,
        numbers.next()??,
        numbers.next()??,
    ];
    numbers.next().is_none().then_some(found)
}

/// `GeoBoundingBox.parseBoundingBox`: `(top, left, bottom, right)`, the
/// parser on the object's start.
fn parse_box(p: &mut Parser<'_>) -> Result<(f64, f64, f64, f64), DslError> {
    let nan = f64::NAN;
    let (mut top, mut bottom, mut left, mut right) = (nan, nan, nan, nan);
    let mut envelope: Option<[f64; 4]> = None;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                // ALLOC: one per request; bounded by the body's recording.
                let name = p.text().to_owned();
                p.next_token();
                match name.as_str() {
                    "wkt" => {
                        let found = wkt_envelope(p.text()).ok_or_else(|| {
                            parse_failure_text("failed to parse WKT bounding box")
                        })?;
                        envelope = Some(found);
                    }
                    "top" => top = double_value(p)?,
                    "bottom" => bottom = double_value(p)?,
                    "left" => left = double_value(p)?,
                    "right" => right = double_value(p)?,
                    "top_left" => (top, left) = corner_point(p, Corner::TopLeft)?,
                    "bottom_right" => (bottom, right) = corner_point(p, Corner::BottomRight)?,
                    "top_right" => (top, right) = corner_point(p, Corner::TopRight)?,
                    "bottom_left" => (bottom, left) = corner_point(p, Corner::BottomLeft)?,
                    other => {
                        // ALLOC: one per request; bounded by the body's recording.
                        return Err(parse_failure_text(format!(
                            "failed to parse bounding box. unexpected field [{other}]"
                        )));
                    }
                }
            }
            Some(other) => {
                // ALLOC: one per request; bounded by the body's recording.
                return Err(parse_failure_text(format!(
                    "failed to parse bounding box. field name expected but [{}] found",
                    other.name()
                )));
            }
        }
    }
    if let Some([min_lon, max_lon, max_lat, min_lat]) = envelope {
        if !(top.is_nan() && bottom.is_nan() && left.is_nan() && right.is_nan()) {
            return Err(parse_failure_text(
                "failed to parse bounding box. Conflicting definition found using well-known text and explicit corners.",
            ));
        }
        if max_lat < min_lat {
            return Err(parse_failure_text("failed to parse WKT bounding box"));
        }
        return Ok((max_lat, min_lon, min_lat, max_lon));
    }
    Ok((top, left, bottom, right))
}

/// `GeoBoundingBoxQueryBuilder.setCorners(double, double, double, double)`
/// as `fromXContent` runs it: before the query's `validation_method` is set,
/// so always strict.
fn check_corners(top: f64, left: f64, bottom: f64, right: f64) -> Result<(), DslError> {
    for (value, what) in [
        (top, "top latitude"),
        (left, "left longitude"),
        (bottom, "bottom latitude"),
        (right, "right longitude"),
    ] {
        if !value.is_finite() {
            // ALLOC: one per request; bounded by the body's recording.
            return Err(DslError::illegal(format!(
                "{what} is invalid: {}",
                java_double(value)
            )));
        }
    }
    let (t, b) = (java_double(top), java_double(bottom));
    if top < bottom {
        // ALLOC: one per request; bounded by the body's recording.
        return Err(DslError::illegal(format!(
            "top is below bottom corner: {t} vs. {b}"
        )));
    }
    if top == bottom {
        // ALLOC: one per request; bounded by the body's recording.
        return Err(DslError::illegal(format!(
            "top cannot be the same as bottom: {t} == {b}"
        )));
    }
    if left == right {
        let (l, r) = (java_double(left), java_double(right));
        // ALLOC: one per request; bounded by the body's recording.
        return Err(DslError::illegal(format!(
            "left cannot be the same as right: {l} == {r}"
        )));
    }
    Ok(())
}

/// `GeoBoundingBoxQueryBuilder.fromXContent`: the parser is on the query
/// object's start.
pub fn parse(p: &mut Parser<'_>) -> Result<GeoBoundingBox, DslError> {
    let mut boost = 1.0;
    // ALLOC: one per request; bounded by the body's recording.
    let mut current = String::new();
    let mut field: Option<String> = None;
    let mut found: Option<(f64, f64, f64, f64)> = None;
    let mut validation: Option<Validation> = None;
    let mut ignore_unmapped = false;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                // ALLOC: one per request; bounded by the body's recording.
                current = p.text().to_owned();
            }
            Some(Kind::StartObject) => {
                found = Some(parse_box(p).map_err(|e| {
                    let reason = e.into_failure().reason;
                    // ALLOC: one per request; bounded by the body's recording.
                    parse_failure_text(format!(
                        "failed to parse [geo_bounding_box] query. [{reason}]"
                    ))
                })?);
                field = Some(current.clone());
            }
            Some(k) if k.is_value() => match current.as_str() {
                "_name" => name_value(p)?,
                "boost" => boost = float_value(p)?,
                "validation_method" => validation = Some(validation_method(p.text())?),
                "ignore_unmapped" => ignore_unmapped = bool_value(p)?,
                "type" => {
                    if !["memory", "indexed"]
                        .iter()
                        .any(|t| t.eq_ignore_ascii_case(p.text()))
                    {
                        // ALLOC: one per request; bounded by the body's recording.
                        return Err(DslError::illegal(format!(
                            "no type can be parsed from ordinal {}",
                            p.text()
                        )));
                    }
                }
                name => {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: one per request; bounded by the body's recording.
                        format!(
                            "failed to parse [geo_bounding_box] query. unexpected field [{name}]"
                        ),
                    ));
                }
            },
            Some(_) => {}
        }
    }
    let (Some(field), Some((top, left, bottom, right))) = (field, found) else {
        return Err(parse_failure_text(
            "failed to parse [geo_bounding_box] query. bounding box not provided",
        ));
    };
    check_corners(top, left, bottom, right)?;
    Ok(GeoBoundingBox {
        field,
        top,
        left,
        bottom,
        right,
        validation: validation.unwrap_or(Validation::Strict),
        ignore_unmapped,
        boost,
    })
}

/// `GeoBoundingBoxQueryBuilder.checkLatLon`.
fn lat_lon_errors(q: &GeoBoundingBox) -> Vec<String> {
    // ALLOC: one per request; bounded by the body's recording.
    let mut errors = Vec::new();
    for (value, what, limit) in [
        (q.top, "top latitude", 90.0),
        (q.left, "left longitude", 180.0),
        (q.bottom, "bottom latitude", 90.0),
        (q.right, "right longitude", 180.0),
    ] {
        if !(value.is_finite() && (-limit..=limit).contains(&value)) {
            // ALLOC: error path; one message.
            errors.push(format!("{what} is invalid: {}", java_double(value)));
        }
    }
    errors
}

/// `GeoBoundingBoxQueryBuilder.doToQuery`.
pub fn build(cx: &QueryContext<'_>, q: &GeoBoundingBox) -> Result<Clause, Failure> {
    let Some(field) = cx.mapping.get(&q.field) else {
        if q.ignore_unmapped {
            return Ok(match_none());
        }
        // ALLOC: error path; one message.
        return Err(shard_failure(format!(
            "failed to find geo field [{}]",
            q.field
        )));
    };
    if field.ty != FieldType::GeoPoint {
        // ALLOC: error path; one message.
        return Err(shard_failure(format!(
            "type [{}] for field [{}] is not supported for [geo_bounding_box] queries. Must be one of [geo_point] or [geo_shape]",
            crate::aggs::metric::resolve::type_name(&field.ty),
            q.field
        )));
    }
    if q.validation == Validation::Strict {
        let errors = lat_lon_errors(q);
        if !errors.is_empty() {
            return Err(validation_failure(&errors));
        }
    }
    let (mut top, mut left, mut bottom, mut right) = (q.top, q.left, q.bottom, q.right);
    if q.validation == Validation::Coerce {
        // Asking for a whole turn of longitude with the right past the left
        // keeps the whole range.
        let complete = (right - left) % 360.0 == 0.0 && right > left;
        let (t, l) = point::normalize(top, left);
        let (b, r) = point::normalize(bottom, right);
        (top, bottom) = (t, b);
        if complete {
            (left, right) = (-180.0, 180.0);
        } else {
            (left, right) = (l, r);
        }
    }
    if top < bottom {
        return Err(Failure::plain(
            "illegal_argument_exception",
            "max y cannot be less than min x",
        ));
    }
    let rectangle = Rectangle::new(bottom, top, left, right)
        // ALLOC: error path; one message.
        .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?;
    let query = lat_lon_point::new_geometry_query(
        &q.field,
        QueryRelation::Intersects,
        &[LatLonGeometry::Rectangle(rectangle)],
    )
    // ALLOC: error path; one message.
    .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?;
    // ALLOC: one key and one box a geo query.
    let key = format!(
        "geo_bounding_box({},{top:?},{left:?},{bottom:?},{right:?})",
        q.field
    );
    // ALLOC: one per request; bounded by the body's recording.
    Ok(Clause::Extended(Box::new(ExtendedQuery::Document(
        DocumentClause::new(key, Arc::from(query)),
    ))))
}
