//! docs/specs/geospatial AC-4 (`geo_centroid` and `geo_bounds`): over three
//! shards, the centroid is the mean of the points and the bounds are the
//! box the two aggregations' rules make of them, wrapped or not, checked
//! against an oracle written here. No stock-server recording exists yet.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: test arithmetic is on small numbers; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use suite_rig::{Cluster, wait_for};

const MAPPING: &str = r#"{"settings":{"index":{"number_of_shards":3}},"mappings":{"properties":{"loc":{"type":"geo_point"},"tag":{"type":"keyword"}}}}"#;

/// Points either side of the antimeridian, then some elsewhere.
fn corpus() -> Vec<(f64, f64)> {
    let mut points = vec![
        (10.0, 170.0),
        (-20.0, 178.5),
        (30.0, -175.0),
        (5.0, -171.25),
    ];
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x >> 11) as f64 / (1u64 << 53) as f64
    };
    // Kept inside the cluster of the first four, so the box stays across
    // the antimeridian: latitudes within -20..30, longitudes 170..180 or
    // -180..-171.25.
    while points.len() < 300 {
        let lon = 170.0 + next() * 18.75;
        let lon = if lon > 180.0 { lon - 360.0 } else { lon };
        points.push((-20.0 + next() * 50.0, lon));
    }
    points
}

fn cluster() -> Cluster {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_GEOSPATIAL_FIELDS", "true");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let made = cluster.call(0, "PUT", "/places", Some(MAPPING));
    assert_eq!(made.status, 200, "{}", made.text());
    let points = corpus();
    for chunk in (0..points.len()).collect::<Vec<_>>().chunks(150) {
        let mut body = String::new();
        for &i in chunk {
            let (lat, lon) = points[i];
            body.push_str(&format!(
                "{{\"index\":{{\"_index\":\"places\",\"_id\":\"{i}\"}}}}\n{{\"loc\":{{\"lat\":{lat},\"lon\":{lon}}},\"tag\":\"t\"}}\n"
            ));
        }
        let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
        assert!(r.text().contains("\"errors\":false"), "{}", r.text());
    }
    // A document with no point, which no geo aggregation counts.
    let r = cluster.call(
        0,
        "PUT",
        "/places/_doc/none?refresh=true",
        Some(r#"{"tag":"t"}"#),
    );
    assert!(r.status < 300, "{}", r.text());
    cluster
}

fn search(cluster: &Cluster, body: &str) -> serde_json::Value {
    let mut out = serde_json::Value::Null;
    wait_for("the aggregation answers", 40, || {
        let r = cluster.call(0, "POST", "/places/_search", Some(body));
        assert_eq!(r.status, 200, "{}", r.text());
        out = serde_json::from_str(&r.text()).expect("json");
        true
    });
    out
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-5
}

fn corner(v: &serde_json::Value, key: &str) -> (f64, f64) {
    (
        v["bounds"][key]["lat"].as_f64().expect("lat"),
        v["bounds"][key]["lon"].as_f64().expect("lon"),
    )
}

#[test]
fn centroid_and_bounds_over_three_shards_match_the_oracle() {
    let cluster = cluster();
    let points = corpus();
    let body = r#"{"size":0,"aggs":{"c":{"geo_centroid":{"field":"loc"}},"b":{"geo_bounds":{"field":"loc"}},"u":{"geo_bounds":{"field":"loc","wrap_longitude":false}}}}"#;
    let got = search(&cluster, body);
    let n = points.len() as f64;
    let lat = points.iter().map(|p| p.0).sum::<f64>() / n;
    let lon = points.iter().map(|p| p.1).sum::<f64>() / n;
    let c = &got["aggregations"]["c"];
    assert_eq!(c["count"], points.len());
    assert!(close(c["location"]["lat"].as_f64().unwrap(), lat), "{c}");
    assert!(close(c["location"]["lon"].as_f64().unwrap(), lon), "{c}");

    let top = points.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let bottom = points.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let pos: Vec<f64> = points.iter().map(|p| p.1).filter(|l| *l >= 0.0).collect();
    let neg: Vec<f64> = points.iter().map(|p| p.1).filter(|l| *l < 0.0).collect();
    let (pos_left, pos_right) = (
        pos.iter().copied().fold(f64::MAX, f64::min),
        pos.iter().copied().fold(f64::MIN, f64::max),
    );
    let (neg_left, neg_right) = (
        neg.iter().copied().fold(f64::MAX, f64::min),
        neg.iter().copied().fold(f64::MIN, f64::max),
    );
    // Wrapped, the box crosses the antimeridian: it starts at the smallest
    // positive longitude and ends at the largest negative one.
    let b = &got["aggregations"]["b"];
    let (tl, br) = (corner(b, "top_left"), corner(b, "bottom_right"));
    assert!(close(tl.0, top) && close(tl.1, pos_left), "{b}");
    assert!(close(br.0, bottom) && close(br.1, neg_right), "{b}");
    // Unwrapped, it spans from the smallest negative to the largest positive.
    let u = &got["aggregations"]["u"];
    let (tl, br) = (corner(u, "top_left"), corner(u, "bottom_right"));
    assert!(close(tl.0, top) && close(tl.1, neg_left), "{u}");
    assert!(close(br.0, bottom) && close(br.1, pos_right), "{u}");
}

#[test]
fn no_matching_point_gives_an_empty_centroid_and_no_bounds() {
    let cluster = cluster();
    let body = r#"{"size":0,"query":{"term":{"tag":"absent"}},"aggs":{"c":{"geo_centroid":{"field":"loc"}},"b":{"geo_bounds":{"field":"loc"}}}}"#;
    let got = search(&cluster, body);
    assert_eq!(got["aggregations"]["c"], serde_json::json!({"count": 0}));
    assert_eq!(got["aggregations"]["b"], serde_json::json!({}));
}

#[test]
fn an_unmapped_field_is_empty_and_a_keyword_field_is_refused() {
    let cluster = cluster();
    let body = r#"{"size":0,"aggs":{"c":{"geo_centroid":{"field":"nowhere"}}}}"#;
    let got = search(&cluster, body);
    assert_eq!(got["aggregations"]["c"], serde_json::json!({"count": 0}));
    let body = r#"{"size":0,"aggs":{"c":{"geo_centroid":{"field":"tag"}}}}"#;
    let r = cluster.call(0, "POST", "/places/_search", Some(body));
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(
        r.text().contains(
            "Field [tag] of type [keyword] is not supported for aggregation [geo_centroid]"
        ),
        "{}",
        r.text()
    );
}
