//! docs/specs/geospatial AC-1 and AC-11: a `geo_point` mapping written
//! through a node, with the switch on and off.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use suite_rig::{Cluster, wait_for};

const MAPPING: &str = r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"title":{"type":"keyword"},"loc":{"type":"geo_point"}}}}"#;

fn cluster(switch: &str) -> Cluster {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_GEOSPATIAL_FIELDS", switch);
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    cluster
}

// docs/specs/geospatial AC-1
#[test]
fn geo_documents_are_written_and_a_malformed_one_is_refused_per_document() {
    let cluster = cluster("true");
    let made = cluster.call(0, "PUT", "/places", Some(MAPPING));
    assert_eq!(made.status, 200, "{}", made.text());
    let body = concat!(
        r#"{"index":{"_index":"places","_id":"1"}}"#,
        "\n",
        r#"{"title":"a","loc":{"lat":41.12,"lon":-71.34}}"#,
        "\n",
        r#"{"index":{"_index":"places","_id":"2"}}"#,
        "\n",
        r#"{"title":"b","loc":"41.12,-71.34"}"#,
        "\n",
        r#"{"index":{"_index":"places","_id":"3"}}"#,
        "\n",
        r#"{"title":"c","loc":{"lat":91,"lon":0}}"#,
        "\n",
    );
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(body));
    let text = r.text();
    assert!(text.contains("\"errors\":true"), "{text}");
    assert!(
        text.contains("failed to parse field [loc] of type [geo_point]"),
        "{text}"
    );
    let got = cluster.call(0, "GET", "/places/_doc/1", None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(got.text().contains(r#""lat":41.12"#), "{}", got.text());
    let gone = cluster.call(0, "GET", "/places/_doc/3", None);
    assert_eq!(gone.status, 404, "{}", gone.text());
}

// docs/specs/geospatial AC-11
#[test]
fn a_geo_point_mapping_is_refused_with_the_switch_off() {
    let cluster = cluster("false");
    let made = cluster.call(0, "PUT", "/places", Some(MAPPING));
    assert_eq!(made.status, 400, "{}", made.text());
    assert!(
        made.text().contains("field type [geo_point]"),
        "{}",
        made.text()
    );
}
