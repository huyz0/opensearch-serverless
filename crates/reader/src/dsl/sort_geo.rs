//! The `_geo_distance` sort: `GeoDistanceSortBuilder.fromXContent`
//! (server/src/main/java/org/opensearch/search/sort/) at OpenSearch 3.10.0.
//! The origins, unit, mode and order are read here; `search_sort_geo` builds
//! the engine's key from them.

use lucene_search::document::geo::DistanceMode;

use crate::dsl::error::DslError;
use crate::dsl::queries::geo_bounding_box::{Corner, corner_point, parse_failure_text};
use crate::dsl::queries::geo_distance::{Validation, double_value, unit_meters, validation_method};
use crate::dsl::sort::{SortKey, order};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::bool_value;
use oss_doc::document::geo as point;

/// A parsed `_geo_distance` sort key.
#[derive(Clone, Debug, PartialEq)]
pub struct GeoDistanceSort {
    /// The field.
    pub field: String,
    /// The origins as `(lat, lon)`, at least one.
    pub points: Vec<(f64, f64)>,
    /// Nearest first.
    pub ascending: bool,
    /// The unit's metres.
    pub unit_meters: f64,
    /// `distance_type: plane`.
    pub plane: bool,
    /// How a document's several distances give one: `mode`, else the
    /// order's own (`min` ascending, `max` descending).
    pub mode: DistanceMode,
    /// `validation_method`.
    pub validation: Validation,
    /// `ignore_unmapped`.
    pub ignore_unmapped: bool,
}

/// `GeoDistanceSortBuilder.parseGeoPoints`: the parser is on the array's
/// start.
fn parse_points(p: &mut Parser<'_>, points: &mut Vec<(f64, f64)>) -> Result<(), DslError> {
    while p.next_token() != Some(Kind::EndArray) {
        if p.current_token() == Some(Kind::Number) {
            let lon = double_value(p)?;
            let next = p.next_token();
            if next != Some(Kind::Number) {
                // ALLOC: one per request; bounded by the body's recording.
                return Err(parse_failure_text(format!(
                    "geo point parsing: expected second number but got [{}] instead",
                    next.map_or("null", Kind::name)
                )));
            }
            let lat = double_value(p)?;
            // ALLOC: one origin of the body's recording.
            points.push((lat, lon));
        } else {
            let found = corner_point(p, Corner::BottomLeft)?;
            // ALLOC: one origin of the body's recording.
            points.push(found);
        }
    }
    Ok(())
}

fn mode(text: &str) -> Result<DistanceMode, DslError> {
    match text.to_lowercase().as_str() {
        "min" => Ok(DistanceMode::Min),
        "max" => Ok(DistanceMode::Max),
        "sum" => Ok(DistanceMode::Sum),
        "avg" => Ok(DistanceMode::Avg),
        "median" => Ok(DistanceMode::Median),
        _ => Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("Unknown SortMode [{text}]"),
        )),
    }
}

fn reset_field(p: &Parser<'_>, current: &str, field: &Option<String>) -> Result<(), DslError> {
    match field {
        Some(first) if first != current => Err(DslError::parsing(
            p.location(),
            // ALLOC: error path; one message.
            format!("Trying to reset fieldName to [{current}], already set to [{first}]."),
        )),
        _ => Ok(()),
    }
}

/// `GeoDistanceSortBuilder.fromXContent`, the parser on the key's object.
pub fn parse(p: &mut Parser<'_>) -> Result<SortKey, DslError> {
    let mut field: Option<String> = None;
    // ALLOC: one origin a point of the body's recording.
    let mut points = Vec::new();
    let mut current = String::new();
    let (mut ascending, mut plane, mut unit) = (true, false, 1.0);
    let mut sort_mode: Option<DistanceMode> = None;
    let mut validation = Validation::Strict;
    let mut ignore_unmapped = false;
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                // ALLOC: one per key; bounded by the body's recording.
                current = p.text().to_owned();
            }
            Some(Kind::StartArray) => {
                parse_points(p, &mut points)?;
                field = Some(current.clone());
            }
            Some(Kind::StartObject) => match current.as_str() {
                "nested_filter" | "nested" => {
                    // ALLOC: one per request; bounded by the body's recording.
                    return Err(DslError::unsupported(&format!(
                        "a [_geo_distance] sort [{current}]"
                    )));
                }
                _ => {
                    reset_field(p, &current, &field)?;
                    field = Some(current.clone());
                    let found = corner_point(p, Corner::BottomLeft)?;
                    // ALLOC: one origin of the body's recording.
                    points.push(found);
                }
            },
            Some(k) if k.is_value() => match current.as_str() {
                "order" => ascending = order(p.text())?,
                "unit" => unit = unit_meters(p.text())?,
                "distance_type" => match p.text().to_lowercase().as_str() {
                    "arc" => plane = false,
                    "plane" => plane = true,
                    other => {
                        return Err(DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("No geo distance for [{other}]"),
                        ));
                    }
                },
                "validation_method" => validation = validation_method(p.text())?,
                "mode" | "sort_mode" => sort_mode = Some(mode(p.text())?),
                "nested_path" => {
                    return Err(DslError::unsupported(
                        "a [_geo_distance] sort [nested_path]",
                    ));
                }
                "ignore_unmapped" => ignore_unmapped = bool_value(p)?,
                _ if k == Kind::String => {
                    reset_field(p, &current, &field)?;
                    let found = point::string_point(p.text(), false)
                        .map_err(|e| parse_failure_text(e.reason))?;
                    // ALLOC: one origin of the body's recording.
                    points.push(found);
                    field = Some(current.clone());
                }
                name if field.as_deref() == Some(name) => {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!("Only geohashes of type string supported for field [{name}]"),
                    ));
                }
                name => {
                    return Err(DslError::parsing(
                        p.location(),
                        // ALLOC: error path; one message.
                        format!("[_geo_distance] does not support [{name}]"),
                    ));
                }
            },
            Some(_) => {}
        }
    }
    if points.is_empty() {
        return Err(DslError::illegal(
            "Geo distance sorting needs at least one point.",
        ));
    }
    let field = field.unwrap_or_default();
    // ALLOC: one per request; bounded by the body's recording.
    Ok(SortKey::GeoDistance(Box::new(GeoDistanceSort {
        field,
        points,
        ascending,
        unit_meters: unit,
        plane,
        mode: sort_mode.unwrap_or(if ascending {
            DistanceMode::Min
        } else {
            DistanceMode::Max
        }),
        validation,
        ignore_unmapped,
    })))
}
