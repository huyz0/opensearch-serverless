//! The `_geo_distance` sort as the engine's key: `GeoDistanceSortBuilder.build`
//! (server/src/main/java/org/opensearch/search/sort/) at OpenSearch 3.10.0,
//! over a `geo_point` field, always with OpenSearch's own comparator: its
//! distances are `SloppyMath.haversinMeters` of the same points as Lucene's
//! `LatLonDocValuesField.newDistanceSort`, so the values read back are the
//! same metres.

use std::sync::Arc;

use lucene_search::document::geo::OpenSearchGeoDistanceSort;
use lucene_search::top_field::SortField;
use oss_blob::BlobStore;
use oss_doc::document::geo as point;

use crate::OpenShard;
use crate::dsl::queries::geo_distance::Validation;
use crate::dsl::sort_geo::GeoDistanceSort;
use crate::failure::Failure;
use crate::mapping::FieldType;
use crate::search::ShardError;
use crate::search_sort::KeyKind;

fn refused(kind: &'static str, reason: String) -> ShardError {
    ShardError::bad_request(Failure::plain(kind, reason))
}

/// `GeoDistanceSortBuilder.build`.
pub(crate) fn geo_sort<S: BlobStore + 'static>(
    sort: &GeoDistanceSort,
    shard: &OpenShard<S>,
) -> Result<(SortField, KeyKind), ShardError> {
    match shard.meta.mapping.get(&sort.field) {
        Some(f) if f.ty == FieldType::GeoPoint => {}
        Some(f) => {
            return Err(refused(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "field [{}] is {}, and a geo distance sort needs a geo_point field",
                    sort.field,
                    f.ty.refusal_name()
                ),
            ));
        }
        None if sort.ignore_unmapped => {}
        None => {
            return Err(refused(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "failed to find mapper for [{}] for geo distance based sort",
                    sort.field
                ),
            ));
        }
    }
    // ALLOC: one origin a point of the body's recording.
    let mut origins = sort.points.clone();
    if sort.validation == Validation::Strict {
        for &(lat, lon) in &origins {
            if !(lat.is_finite() && (-90.0..=90.0).contains(&lat)) {
                return Err(refused(
                    "opensearch_parse_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "illegal latitude value [{}] for [GeoDistanceSort] for field [{}].",
                        oss_doc::mapping::json::java_double(lat),
                        sort.field
                    ),
                ));
            }
            if !(lon.is_finite() && (-180.0..=180.0).contains(&lon)) {
                return Err(refused(
                    "opensearch_parse_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "illegal longitude value [{}] for [GeoDistanceSort] for field [{}].",
                        oss_doc::mapping::json::java_double(lon),
                        sort.field
                    ),
                ));
            }
        }
    }
    if sort.validation == Validation::Coerce {
        for origin in &mut origins {
            *origin = point::normalize(origin.0, origin.1);
        }
    }
    let source = OpenSearchGeoDistanceSort {
        origins,
        unit_meters: sort.unit_meters,
        mode: sort.mode,
        plane: sort.plane,
    };
    Ok((
        // ALLOC: one comparator source a sort key.
        SortField::with_source(&sort.field, Arc::new(source), !sort.ascending),
        KeyKind::Double,
    ))
}
