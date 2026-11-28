//! docs/specs/geospatial AC-2 (the `geo_distance` part): the ids a
//! `geo_distance` query returns over a fixed corpus of 1,000 points, the
//! dateline and the poles among them, equal those of a brute-force
//! haversine oracle written here. No stock-server recording exists yet.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: test arithmetic is on small numbers; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use suite_rig::{Cluster, wait_for};

const MAPPING: &str = r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"loc":{"type":"geo_point"},"other":{"type":"keyword"}}}}"#;

/// `GeoUtils.EARTH_MEAN_RADIUS_METERS`.
const EARTH: f64 = 6_371_008.771_4;

/// The corpus: pseudo-random points, then the places a distance query goes
/// wrong: the poles, the dateline on both sides and the origin.
fn corpus() -> Vec<(f64, f64)> {
    let mut points = vec![
        (90.0, 0.0),
        (-90.0, 0.0),
        (89.999, 120.0),
        (0.0, 0.0),
        (0.0, 180.0),
        (0.0, -180.0),
        (10.0, 179.9999),
        (10.0, -179.9999),
        (-10.0, 179.5),
        (-10.0, -179.5),
    ];
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        // 53 random bits as a fraction of one.
        (x >> 11) as f64 / (1u64 << 53) as f64
    };
    while points.len() < 700 {
        points.push((next() * 180.0 - 90.0, next() * 360.0 - 180.0));
    }
    // A dense cluster near the dateline and near the north pole.
    while points.len() < 850 {
        let lon = 175.0 + next() * 10.0;
        let lon = if lon > 180.0 { lon - 360.0 } else { lon };
        points.push((next() * 20.0 - 10.0, lon));
    }
    while points.len() < 1_000 {
        points.push((85.0 + next() * 5.0, next() * 360.0 - 180.0));
    }
    points
}

/// Haversine metres.
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = (b.0 - a.0).to_radians();
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH * h.sqrt().min(1.0).asin()
}

fn ids(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("\"_id\":\"") {
        rest = &rest[at + 7..];
        let end = rest.find('"').expect("closes");
        out.push(rest[..end].parse().expect("numeric id"));
    }
    out.sort_unstable();
    out
}

fn search(cluster: &Cluster, query: &str) -> Result<Vec<usize>, String> {
    let body = format!(r#"{{"size":1000,"_source":false,"query":{query}}}"#);
    let r = cluster.call(0, "POST", "/places/_search", Some(&body));
    if r.status == 200 {
        Ok(ids(&r.text()))
    } else {
        Err(r.text())
    }
}

fn cluster() -> Cluster {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_GEOSPATIAL_FIELDS", "true");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    let made = cluster.call(0, "PUT", "/places", Some(MAPPING));
    assert_eq!(made.status, 200, "{}", made.text());
    let points = corpus();
    for chunk in (0..points.len()).collect::<Vec<_>>().chunks(250) {
        let mut body = String::new();
        for &i in chunk {
            let (lat, lon) = points[i];
            body.push_str(&format!(
                "{{\"index\":{{\"_index\":\"places\",\"_id\":\"{i}\"}}}}\n{{\"loc\":{{\"lat\":{lat},\"lon\":{lon}}}}}\n"
            ));
        }
        let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
        assert!(r.text().contains("\"errors\":false"), "{}", r.text());
    }
    cluster
}

// docs/specs/geospatial AC-2
#[test]
fn geo_distance_returns_the_oracles_ids_including_the_dateline_and_the_poles() {
    let cluster = cluster();
    let points = corpus();
    // Centre, radius in metres: ordinary, across the dateline, at a pole, tiny, huge.
    let cases: [((f64, f64), f64); 8] = [
        ((10.0, 20.0), 2_000_000.0),
        ((0.0, 179.9), 900_000.0),
        ((0.0, -179.9), 900_000.0),
        ((90.0, 0.0), 1_500_000.0),
        ((-90.0, 45.0), 3_000_000.0),
        ((0.0, 0.0), 1.0),
        ((45.0, 90.0), 12_000_000.0),
        ((88.0, -100.0), 800_000.0),
    ];
    let mut compared = 0usize;
    for ((lat, lon), radius) in cases {
        let q = format!(
            r#"{{"geo_distance":{{"distance":"{radius}m","loc":{{"lat":{lat},"lon":{lon}}}}}}}"#
        );
        let got = wait_for_ids(&cluster, &q);
        // Points within a metre of the edge depend on the 1e-7 degree encoding: left out.
        let mut near_edge = Vec::new();
        let mut want = Vec::new();
        for (i, p) in points.iter().enumerate() {
            let d = distance((lat, lon), *p);
            if (d - radius).abs() < 1.0 {
                near_edge.push(i);
            } else if d < radius {
                want.push(i);
            }
        }
        let got: Vec<usize> = got.into_iter().filter(|i| !near_edge.contains(i)).collect();
        assert_eq!(got, want, "centre ({lat}, {lon}) radius {radius}");
        compared += want.len();
    }
    assert!(compared > 300, "the cases match something: {compared}");
}

fn wait_for_ids(cluster: &Cluster, query: &str) -> Vec<usize> {
    let mut got = None;
    wait_for("the query answers", 40, || match search(cluster, query) {
        Ok(v) => {
            got = Some(v);
            true
        }
        Err(e) => panic!("{e}"),
    });
    got.expect("answered")
}

// docs/specs/geospatial AC-2
#[test]
fn geo_distance_refuses_a_bad_centre_a_non_geo_field_and_honours_ignore_unmapped() {
    let cluster = cluster();
    let bad = search(
        &cluster,
        r#"{"geo_distance":{"distance":"1km","loc":{"lat":91,"lon":0}}}"#,
    )
    .expect_err("strict validation");
    assert!(
        bad.contains("couldn't validate latitude/ longitude values"),
        "{bad}"
    );
    let coerced = search(
        &cluster,
        r#"{"geo_distance":{"distance":"1km","validation_method":"COERCE","loc":{"lat":91,"lon":0}}}"#,
    );
    assert!(coerced.is_ok(), "{coerced:?}");
    let other = search(
        &cluster,
        r#"{"geo_distance":{"distance":"1km","other":{"lat":1,"lon":0}}}"#,
    )
    .expect_err("not a geo field");
    assert!(
        other.contains("is not supported for [geo_distance] queries"),
        "{other}"
    );
    let missing = search(
        &cluster,
        r#"{"geo_distance":{"distance":"1km","nope":{"lat":1,"lon":0}}}"#,
    )
    .expect_err("unmapped");
    assert!(
        missing.contains("failed to find geo field [nope]"),
        "{missing}"
    );
    let ignored = search(
        &cluster,
        r#"{"geo_distance":{"distance":"1km","ignore_unmapped":true,"nope":{"lat":1,"lon":0}}}"#,
    )
    .expect("ignored");
    assert!(ignored.is_empty());
    let no_distance = search(&cluster, r#"{"geo_distance":{"loc":{"lat":1,"lon":0}}}"#)
        .expect_err("distance required");
    assert!(no_distance.contains("requires 'distance'"), "{no_distance}");
}
