//! The `geo_polygon` query: `GeoPolygonQueryBuilder.fromXContent` and
//! `doToQuery` (server/src/main/java/org/opensearch/index/query/) at
//! OpenSearch 3.10.0, over a `geo_point` field: `LatLonPoint.newPolygonQuery`.

use std::sync::Arc;

use lucene_search::Clause;
use lucene_search::document::geo::lat_lon_point;
use lucene_search::extended_query::{DocumentClause, ExtendedQuery};
use lucene_util::geo::Polygon;
use oss_doc::document::geo as point;

use super::geo_bounding_box::{Corner, corner_point};
use super::geo_distance::{Validation, shard_failure, validation_method};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::{bool_value, float_value, name_value};
use crate::failure::Failure;
use crate::lucene_query::{QueryContext, match_none};
use crate::mapping::FieldType;
use oss_doc::mapping::json::java_double;

/// A parsed `geo_polygon` query.
#[derive(Clone, Debug, PartialEq)]
pub struct GeoPolygon {
    /// The field.
    pub field: String,
    /// The shell as `(lat, lon)`, closed.
    pub shell: Vec<(f64, f64)>,
    /// `validation_method`.
    pub validation: Validation,
    /// `ignore_unmapped`.
    pub ignore_unmapped: bool,
    /// The boost.
    pub boost: f32,
}

fn unsupported_param(p: &Parser<'_>, name: &str) -> DslError {
    DslError::parsing(
        p.location(),
        // ALLOC: error path; one message.
        format!("[geo_polygon] query does not support [{name}]"),
    )
}

fn token_name(kind: Option<Kind>) -> &'static str {
    kind.map_or("null", Kind::name)
}

/// The `points` array: one point in any form each.
fn parse_points(p: &mut Parser<'_>) -> Result<Vec<(f64, f64)>, DslError> {
    // ALLOC: one point per element of the body's recording.
    let mut shell = Vec::new();
    while p.next_token() != Some(Kind::EndArray) {
        let found = corner_point(p, Corner::BottomLeft)?;
        // ALLOC: one point per element of the body's recording.
        shell.push(found);
    }
    Ok(shell)
}

/// `new GeoPolygonQueryBuilder(fieldName, points)`: the checks and the
/// closing of the shell.
fn close(
    field: Option<String>,
    points: Option<Vec<(f64, f64)>>,
) -> Result<(String, Vec<(f64, f64)>), DslError> {
    let field = field
        .filter(|f| !f.is_empty())
        .ok_or_else(|| DslError::illegal("fieldName must not be null"))?;
    let mut shell = points
        .filter(|s| !s.is_empty())
        .ok_or_else(|| DslError::illegal("polygon must not be null or empty"))?;
    let closed = shell.first() == shell.last();
    let least = if closed { 4 } else { 3 };
    if shell.len() < least {
        return Err(DslError::illegal(
            "too few points defined for geo_polygon query",
        ));
    }
    if !closed && let Some(first) = shell.first().copied() {
        shell.push(first);
    }
    Ok((field, shell))
}

/// `GeoPolygonQueryBuilder.fromXContent`: the parser is on the query
/// object's start.
pub fn parse(p: &mut Parser<'_>) -> Result<GeoPolygon, DslError> {
    let mut boost = 1.0;
    // ALLOC: one per request; bounded by the body's recording.
    let mut current = String::new();
    let mut field: Option<String> = None;
    let mut points: Option<Vec<(f64, f64)>> = None;
    let mut validation = Validation::Strict;
    let mut ignore_unmapped = false;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                // ALLOC: one per request; bounded by the body's recording.
                current = p.text().to_owned();
            }
            Some(Kind::StartObject) => {
                field = Some(current.clone());
                // ALLOC: one per request; bounded by the body's recording.
                let mut inner = String::new();
                loop {
                    match p.next_token() {
                        Some(Kind::EndObject) | None => break,
                        Some(Kind::FieldName) => {
                            // ALLOC: one per request; bounded by the body's recording.
                            inner = p.text().to_owned();
                        }
                        Some(Kind::StartArray) => {
                            if inner == "points" {
                                points = Some(parse_points(p)?);
                            } else {
                                return Err(unsupported_param(p, &inner));
                            }
                        }
                        other => {
                            return Err(DslError::parsing(
                                p.location(),
                                // ALLOC: one per request; bounded by the body's recording.
                                format!(
                                    "[geo_polygon] query does not support token type [{}] under [{inner}]",
                                    token_name(other)
                                ),
                            ));
                        }
                    }
                }
            }
            Some(k) if k.is_value() => match current.as_str() {
                "_name" => name_value(p)?,
                "boost" => boost = float_value(p)?,
                "ignore_unmapped" => ignore_unmapped = bool_value(p)?,
                "validation_method" => validation = validation_method(p.text())?,
                name => return Err(unsupported_param(p, name)),
            },
            other => {
                return Err(DslError::parsing(
                    p.location(),
                    // ALLOC: one per request; bounded by the body's recording.
                    format!(
                        "[geo_polygon] unexpected token type [{}]",
                        token_name(other)
                    ),
                ));
            }
        }
    }
    let (field, shell) = close(field, points)?;
    Ok(GeoPolygon {
        field,
        shell,
        validation,
        ignore_unmapped,
        boost,
    })
}

/// `GeoPolygonQueryBuilder.doToQuery`.
pub fn build(cx: &QueryContext<'_>, q: &GeoPolygon) -> Result<Clause, Failure> {
    let Some(field) = cx.mapping.get(&q.field) else {
        if q.ignore_unmapped {
            return Ok(match_none());
        }
        // ALLOC: error path; one message.
        return Err(shard_failure(format!(
            "failed to find geo_point field [{}]",
            q.field
        )));
    };
    if field.ty != FieldType::GeoPoint {
        // ALLOC: error path; one message.
        return Err(shard_failure(format!(
            "field [{}] is not a geo_point field",
            q.field
        )));
    }
    // ALLOC: one point per vertex of the body's recording.
    let mut shell = q.shell.clone();
    if q.validation == Validation::Strict {
        for &(lat, lon) in &shell {
            if !(lat.is_finite() && (-90.0..=90.0).contains(&lat)) {
                // ALLOC: error path; one message.
                return Err(shard_failure(format!(
                    "illegal latitude value [{}] for [geo_polygon]",
                    java_double(lat)
                )));
            }
            if !(lon.is_finite() && (-180.0..=180.0).contains(&lon)) {
                // ALLOC: error path; one message.
                return Err(shard_failure(format!(
                    "illegal longitude value [{}] for [geo_polygon]",
                    java_double(lon)
                )));
            }
        }
    }
    if q.validation == Validation::Coerce {
        for vertex in &mut shell {
            *vertex = point::normalize(vertex.0, vertex.1);
        }
    }
    // ALLOC: one coordinate per vertex of the body's recording.
    let lats: Vec<f64> = shell.iter().map(|v| v.0).collect();
    // ALLOC: one coordinate per vertex of the body's recording.
    let lons: Vec<f64> = shell.iter().map(|v| v.1).collect();
    let polygon = Polygon::from_vecs(lats, lons, Vec::new())
        // ALLOC: error path; one message.
        .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?;
    let query = lat_lon_point::new_polygon_query(&q.field, &[polygon])
        // ALLOC: error path; one message.
        .map_err(|e| Failure::plain("illegal_argument_exception", e.to_string()))?;
    // ALLOC: one key a geo query, a vertex wide.
    let key = format!("geo_polygon({},{:?})", q.field, shell);
    Ok(Clause::Extended(Box::new(ExtendedQuery::Document(
        DocumentClause::new(key, Arc::from(query)),
    ))))
}
