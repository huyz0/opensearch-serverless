//! The `geo_distance` query: `GeoDistanceQueryBuilder.fromXContent` and
//! `doToQuery` (server/src/main/java/org/opensearch/index/query/) at
//! OpenSearch 3.10.0, over a `geo_point` field as
//! `VectorGeoPointShapeQueryProcessor` builds it: a circle intersected, which
//! is `LatLonPoint.newDistanceQuery`.

use std::sync::Arc;

use lucene_search::Clause;
use lucene_search::document::geo::lat_lon_point;
use lucene_search::extended_query::{DocumentClause, ExtendedQuery};
use oss_doc::document::geo as point;

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, float_value, name_value};
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, match_none};
use crate::mapping::FieldType;

/// `GeoValidationMethod`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validation {
    /// Wrap an out-of-range centre into range.
    Coerce,
    /// Accept an out-of-range centre as it is.
    IgnoreMalformed,
    /// Refuse an out-of-range centre; the default.
    Strict,
}

/// A parsed `geo_distance` query.
#[derive(Clone, Debug, PartialEq)]
pub struct GeoDistance {
    /// The field.
    pub field: String,
    /// The centre's latitude.
    pub lat: f64,
    /// The centre's longitude.
    pub lon: f64,
    /// The radius in metres.
    pub meters: f64,
    /// `validation_method`.
    pub validation: Validation,
    /// `ignore_unmapped`.
    pub ignore_unmapped: bool,
    /// The boost.
    pub boost: f32,
}

/// `DistanceUnit`'s names and metres, in the Java's order (which decides
/// which suffix a distance string matches).
const UNITS: &[(&[&str], f64)] = &[
    (&["in", "inch"], 0.0254),
    (&["yd", "yards"], 0.9144),
    (&["ft", "feet"], 0.3048),
    (&["km", "kilometers"], 1000.0),
    (&["NM", "nmi", "nauticalmiles"], 1852.0),
    (&["mm", "millimeters"], 0.001),
    (&["cm", "centimeters"], 0.01),
    (&["mi", "miles"], 1609.344),
    (&["m", "meters"], 1.0),
];

/// `DistanceUnit.fromString`: the metres in the unit `name`.
pub(crate) fn unit_meters(name: &str) -> Result<f64, DslError> {
    UNITS
        .iter()
        .find(|(names, _)| names.contains(&name))
        .map(|(_, m)| *m)
        // ALLOC: error path; one message.
        .ok_or_else(|| DslError::illegal(format!("No distance unit match [{name}]")))
}

/// `Double.parseDouble`'s refusal.
fn number_format(text: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::Failure(Failure::plain(
        "number_format_exception",
        if text.trim().is_empty() {
            // ALLOC: one per request; bounded by the body's recording.
            "empty String".to_owned()
        } else {
            // ALLOC: one per request; bounded by the body's recording.
            format!("For input string: \"{text}\"")
        },
    ))
}

fn java_double(text: &str) -> Result<f64, DslError> {
    oss_doc::mapping::java_parse_double(text).ok_or_else(|| number_format(text))
}

/// `DistanceUnit.parse(distance, unit, DEFAULT)`: metres.
fn parse_distance(text: &str, default_unit: f64) -> Result<f64, DslError> {
    for (names, meters) in UNITS {
        for name in *names {
            if let Some(number) = text.strip_suffix(name) {
                return Ok(java_double(number)? * meters);
            }
        }
    }
    Ok(java_double(text)? * default_unit)
}

pub(crate) fn double_value(p: &Parser<'_>) -> Result<f64, DslError> {
    match p.current_token() {
        Some(Kind::Number | Kind::String) => java_double(p.text().trim()),
        _ => Err(number_format(p.text())),
    }
}

/// `GeoUtils.parseGeoPoint(parser, point)` of an array, at its `[`.
pub(super) fn parse_array(p: &mut Parser<'_>) -> Result<(f64, f64), DslError> {
    let (mut x, mut y, mut n) = (f64::NAN, f64::NAN, 0usize);
    while p.next_token() != Some(Kind::EndArray) {
        if p.current_token() != Some(Kind::Number) {
            return Err(parse_failure(p, "numeric value expected"));
        }
        n = n.saturating_add(1);
        match n {
            1 => x = double_value(p)?,
            2 => y = double_value(p)?,
            _ => {
                return Err(parse_failure(
                    p,
                    // ALLOC: one per request; bounded by the body's recording.
                    format!(
                        "Exception parsing coordinates: found Z value [{}] but [ignore_z_value] parameter is [false]",
                        p.text()
                    ),
                ));
            }
        }
    }
    if n < 2 {
        return Err(parse_failure(
            p,
            "[geo_point] field type should have at least two dimensions",
        ));
    }
    Ok((y, x))
}

pub(super) fn parse_failure(_p: &Parser<'_>, reason: impl Into<String>) -> DslError {
    DslError::Failure(Failure::plain("opensearch_parse_exception", reason))
}

/// `"geo_distance"`'s refusal of an unexpected field.
fn unexpected(p: &Parser<'_>, name: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("failed to parse [geo_distance] query. unexpected field [{name}]"),
    )
}

/// `GeoDistanceQueryBuilder.fromXContent`: the parser is on the query
/// object's start.
pub fn parse(p: &mut Parser<'_>) -> Result<GeoDistance, DslError> {
    let mut boost = 1.0;
    let mut current = "";
    let (mut lat, mut lon) = (f64::NAN, f64::NAN);
    let mut field: Option<String> = None;
    let mut distance: Option<String> = None;
    let mut unit = 1.0;
    let mut validation = Validation::Strict;
    let mut ignore_unmapped = false;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => current = p.text(),
            Some(Kind::StartArray) => {
                // ALLOC: one per request; bounded by the body's recording.
                field = Some(current.to_owned());
                (lat, lon) = parse_array(p)?;
            }
            Some(Kind::StartObject) => {
                if let Some(first) = &field {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!(
                            "[geo_distance] query doesn't support multiple fields, found [{first}] and [{current}]"
                        ),
                    ));
                }
                // ALLOC: one per request; bounded by the body's recording.
                field = Some(current.to_owned());
                let mut inner = current;
                while let Some(k) = p.next_token() {
                    match k {
                        Kind::EndObject => break,
                        Kind::FieldName => inner = p.text(),
                        k if k.is_value() => match inner {
                            "lat" => lat = double_value(p)?,
                            "lon" => lon = double_value(p)?,
                            "geohash" => (lat, lon) = geohash(p)?,
                            _ => {
                                return Err(DslError::parsing(
                                    p.location(),
                                    // ALLOC: error path; one message.
                                    format!("[geo_distance] query does not support [{current}]"),
                                ));
                            }
                        },
                        _ => {}
                    }
                }
            }
            Some(k) if k.is_value() => match current {
                "distance" => {
                    // ALLOC: one number or string a query.
                    distance = Some(p.text().to_owned());
                    if p.current_token() == Some(Kind::Number)
                        && !p.text().contains(['.', 'e', 'E'])
                    {
                        // `Double.toString` of an integer ends in `.0`.
                        distance = distance.map(|d| d + ".0");
                    }
                }
                "unit" => unit = unit_meters(p.text())?,
                "distance_type" => match p.text().to_lowercase().as_str() {
                    "plane" | "arc" => {}
                    other => {
                        return Err(DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("No geo distance for [{other}]"),
                        ));
                    }
                },
                name if name.ends_with(".lat") => {
                    lat = double_value(p)?;
                    // ALLOC: one per request; bounded by the body's recording.
                    field = Some(name.trim_end_matches(".lat").to_owned());
                }
                name if name.ends_with(".lon") => {
                    lon = double_value(p)?;
                    // ALLOC: one per request; bounded by the body's recording.
                    field = Some(name.trim_end_matches(".lon").to_owned());
                }
                "_name" => name_value(p)?,
                "boost" => boost = float_value(p)?,
                "ignore_unmapped" => ignore_unmapped = bool_value(p)?,
                "validation_method" => validation = validation_method(p.text())?,
                name if field.is_none() => {
                    (lat, lon) = point::string_point(p.text(), false)
                        .map_err(|e| DslError::illegal(e.reason))?;
                    // ALLOC: one per request; bounded by the body's recording.
                    field = Some(name.to_owned());
                }
                name => return Err(unexpected(p, name)),
            },
            Some(_) => {}
        }
    }
    let Some(distance) = distance else {
        return Err(DslError::parsing(
            p.location(),
            "geo_distance requires 'distance' to be specified",
        ));
    };
    let field = field.unwrap_or_default();
    if field.is_empty() {
        return Err(DslError::illegal("fieldName must not be null or empty"));
    }
    if distance.is_empty() {
        return Err(DslError::illegal("distance must not be null or empty"));
    }
    let meters = parse_distance(&distance, unit)?;
    if meters <= 0.0 {
        return Err(DslError::illegal("distance must be greater than zero"));
    }
    Ok(GeoDistance {
        field,
        lat,
        lon,
        meters,
        validation,
        ignore_unmapped,
        boost,
    })
}

fn geohash(p: &Parser<'_>) -> Result<(f64, f64), DslError> {
    point::geohash(p.text()).map_err(|e| DslError::illegal(e.reason))
}

/// `GeoValidationMethod.fromString`.
pub(crate) fn validation_method(text: &str) -> Result<Validation, DslError> {
    match text.to_uppercase().as_str() {
        "COERCE" => Ok(Validation::Coerce),
        "IGNORE_MALFORMED" => Ok(Validation::IgnoreMalformed),
        "STRICT" => Ok(Validation::Strict),
        _ => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!(
                "operator needs to be either [COERCE, IGNORE_MALFORMED, STRICT], but not [{text}]"
            ),
        )),
    }
}

pub(super) fn shard_failure(reason: impl Into<String>) -> Failure {
    Failure::plain("query_shard_exception", reason)
}

/// The shard failure `QueryShardException(context, "couldn't validate
/// latitude/ longitude values", exception)` makes of the numbered errors.
pub(super) fn validation_failure(errors: &[String]) -> Failure {
    let numbered: String = errors
        .iter()
        .enumerate()
        // ALLOC: one per request; bounded by the body's recording.
        .map(|(i, e)| format!("{}: {e};", i.saturating_add(1)))
        // ALLOC: one string per request; bounded by the body's recording.
        .collect();
    shard_failure("couldn't validate latitude/ longitude values").caused_by(Failure::plain(
        "validation_exception",
        // ALLOC: error path; one message.
        format!("Validation Failed: {numbered}"),
    ))
}

/// `GeoDistanceQueryBuilder.doToQuery`.
pub fn build(cx: &QueryContext<'_>, q: &GeoDistance) -> Result<Clause, Failure> {
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
            "type [{}] for field [{}] is not supported for [geo_distance] queries. Must be one of [geo_point] or [geo_shape]",
            crate::aggs::metric::resolve::type_name(&field.ty),
            q.field
        )));
    }
    let (mut lat, mut lon) = (q.lat, q.lon);
    if q.validation == Validation::Strict {
        // ALLOC: one per request; bounded by the body's recording.
        let mut errors = Vec::new();
        if !(lat.is_finite() && (-90.0..=90.0).contains(&lat)) {
            // ALLOC: error path; one message.
            errors.push(format!("center point latitude is invalid: {lat:?}"));
        }
        if !(lon.is_finite() && (-180.0..=180.0).contains(&lon)) {
            // ALLOC: error path; one message.
            errors.push(format!("center point longitude is invalid: {lon:?}"));
        }
        if !errors.is_empty() {
            return Err(validation_failure(&errors));
        }
    }
    if q.validation == Validation::Coerce {
        (lat, lon) = point::normalize(lat, lon);
    }
    let query = lat_lon_point::new_distance_query(&q.field, lat, lon, q.meters)
        // ALLOC: error path; one message.
        .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?;
    // ALLOC: one key and one box a geo query.
    let key = format!("geo_distance({},{lat:?},{lon:?},{:?})", q.field, q.meters);
    Ok(Clause::Extended(Box::new(ExtendedQuery::Document(
        DocumentClause::new(key, Arc::from(query)),
    ))))
}
