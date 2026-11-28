//! The geo fixtures against an assembled node: `geo_bounds` and
//! `geo_centroid` answers, and what indexing a `geo_point` accepts and refuses.

use super::{Fixture, load_dir, replay_geo, root};

/// The steps of the geo aggregation fixture a node serves: the setup, and
/// every `geo_bounds` and `geo_centroid` step but those that use a feature
/// the node refuses as not served yet: `missing`, `meta`, a sub-aggregation
/// of `terms` or `filter`, and a grid.
fn serves_geo_metric(name: &str) -> bool {
    const NOT_YET: [&str; 4] = ["missing value", "meta", "under ", "of a grid"];
    name.starts_with("create ")
        || name.starts_with("index ")
        || ((name.starts_with("geo_bounds") || name.starts_with("geo_centroid"))
            && !NOT_YET.iter().any(|w| name.contains(w)))
}

#[test]
fn an_assembled_node_answers_the_geo_metric_aggregations_as_stock_opensearch_did() {
    let mut fixtures: Vec<Fixture> = load_dir(&root().join("compat/geo"))
        .expect("fixtures load")
        .into_iter()
        .filter(|(p, _)| p.file_name().is_some_and(|n| n == "aggs-geo.json"))
        .map(|(_, f)| f)
        .collect();
    assert_eq!(fixtures.len(), 1, "the geo aggregation fixture");
    let mut fixture = fixtures.remove(0);
    fixture.steps.retain(|s| serves_geo_metric(&s.name));
    assert!(fixture.steps.len() > 40, "bounds and centroid steps");
    let failed = replay_geo(fixture, false, false, false, true);
    assert!(
        failed.is_empty(),
        "{} differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

/// The steps of the geo point values fixture a node serves: the index, its
/// mapping, every document that indexes, and every one that is refused but
/// the one whose refusal quotes a JSON parser's byte offset.
fn serves_geo_point_values(name: &str) -> bool {
    name.starts_with("create ")
        || name == "mapping"
        || name.starts_with("index: ")
        || (name.starts_with("refused: ") && name != "refused: one element array")
}

#[test]
fn an_assembled_node_indexes_and_refuses_geo_points_as_stock_opensearch_did() {
    let mut fixture = load_dir(&root().join("compat/geo"))
        .expect("fixtures load")
        .into_iter()
        .find(|(p, _)| p.file_name().is_some_and(|n| n == "point-values.json"))
        .map(|(_, f)| f)
        .expect("the point values fixture");
    fixture.steps.retain(|s| serves_geo_point_values(&s.name));
    assert!(fixture.steps.len() > 50, "the index and refusal steps");
    let failed = replay_geo(fixture, false, false, false, true);
    assert!(
        failed.is_empty(),
        "{} differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
