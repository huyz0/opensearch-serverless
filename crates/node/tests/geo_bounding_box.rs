//! docs/specs/geospatial AC-2 (the `geo_bounding_box` part): the ids a
//! `geo_bounding_box` query returns over a fixed corpus of 1,000 points, the
//! dateline and the poles among them, equal those of a brute-force box test
//! written here. No stock-server recording exists yet.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: test arithmetic is on small numbers; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use suite_rig::{Cluster, wait_for};

const MAPPING: &str = r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"loc":{"type":"geo_point"},"other":{"type":"keyword"}}}}"#;

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

/// Whether the box holds the point; a box whose right is left of its left
/// crosses the dateline.
fn inside(top: f64, left: f64, bottom: f64, right: f64, p: (f64, f64)) -> bool {
    let lat = p.0 >= bottom && p.0 <= top;
    let lon = if left <= right {
        p.1 >= left && p.1 <= right
    } else {
        p.1 >= left || p.1 <= right
    };
    lat && lon
}

/// Whether a point is within a hundred-thousandth of a degree of an edge,
/// where the 1e-7 degree encoding may decide it either way.
fn near_edge(top: f64, left: f64, bottom: f64, right: f64, p: (f64, f64)) -> bool {
    let near = |a: f64, b: f64| (a - b).abs() < 1e-5;
    near(p.0, top) || near(p.0, bottom) || near(p.1, left) || near(p.1, right)
}

// docs/specs/geospatial AC-2
#[test]
fn geo_bounding_box_returns_the_oracles_ids_including_the_dateline_and_the_poles() {
    let cluster = cluster();
    let points = corpus();
    // top, left, bottom, right: ordinary, across the dateline, polar, the
    // whole globe, a sliver, and a box that crosses both.
    let cases: [(f64, f64, f64, f64); 8] = [
        (40.0, -30.0, -20.0, 60.0),
        (30.0, 170.0, -30.0, -170.0),
        (90.0, -180.0, 80.0, 180.0),
        (90.0, -180.0, -90.0, 180.0),
        (0.5, 0.0, -0.5, 1.0),
        (10.0, 176.0, -10.0, 179.0),
        (60.0, 100.0, 10.0, -100.0),
        (-60.0, -180.0, -90.0, 0.0),
    ];
    let mut compared = 0usize;
    for (top, left, bottom, right) in cases {
        let forms = [
            format!(
                r#"{{"geo_bounding_box":{{"loc":{{"top_left":{{"lat":{top},"lon":{left}}},"bottom_right":{{"lat":{bottom},"lon":{right}}}}}}}}}"#
            ),
            format!(
                r#"{{"geo_bounding_box":{{"loc":{{"top":{top},"left":{left},"bottom":{bottom},"right":{right}}}}}}}"#
            ),
            format!(
                r#"{{"geo_bounding_box":{{"loc":{{"top_left":[{left},{top}],"bottom_right":"{bottom},{right}"}}}}}}"#
            ),
            format!(
                r#"{{"geo_bounding_box":{{"loc":{{"wkt":"BBOX ({left}, {right}, {top}, {bottom})"}}}}}}"#
            ),
        ];
        let mut want = Vec::new();
        let mut edge = Vec::new();
        for (i, p) in points.iter().enumerate() {
            if near_edge(top, left, bottom, right, *p) {
                edge.push(i);
            } else if inside(top, left, bottom, right, *p) {
                want.push(i);
            }
        }
        for q in &forms {
            let got: Vec<usize> = wait_for_ids(&cluster, q)
                .into_iter()
                .filter(|i| !edge.contains(i))
                .collect();
            assert_eq!(got, want, "{q}");
        }
        compared += want.len();
    }
    assert!(compared > 1_000, "the cases match something: {compared}");
}

// docs/specs/geospatial AC-2
#[test]
fn geo_bounding_box_refuses_bad_boxes_and_honours_coerce_and_ignore_unmapped() {
    let cluster = cluster();
    let all = |q: &str| search(&cluster, q);
    let strict = all(r#"{"geo_bounding_box":{"loc":{"top_left":{"lat":91,"lon":0},"bottom_right":{"lat":0,"lon":10}}}}"#)
        .expect_err("a latitude past 90 is refused");
    assert!(
        strict.contains("couldn't validate latitude/ longitude values"),
        "{strict}"
    );
    assert!(strict.contains("top latitude is invalid: 91.0"), "{strict}");
    let upside = all(r#"{"geo_bounding_box":{"loc":{"top":0,"left":0,"bottom":10,"right":10}}}"#)
        .expect_err("top below bottom");
    assert!(
        upside.contains("top is below bottom corner: 0.0 vs. 10.0"),
        "{upside}"
    );
    // A whole turn of longitude with COERCE is the whole range.
    let whole = all(
        r#"{"geo_bounding_box":{"validation_method":"COERCE","loc":{"top":90,"left":-180,"bottom":-90,"right":180}}}"#,
    )
    .expect("coerce");
    assert_eq!(whole.len(), 1_000);
    let wrapped = all(
        r#"{"geo_bounding_box":{"validation_method":"COERCE","loc":{"top":90,"left":0,"bottom":-90,"right":360}}}"#,
    )
    .expect("coerce wraps a whole turn");
    assert_eq!(wrapped.len(), 1_000);
    let other = all(r#"{"geo_bounding_box":{"other":{"top":1,"left":0,"bottom":0,"right":1}}}"#)
        .expect_err("a keyword field is not a geo field");
    assert!(
        other.contains("is not supported for [geo_bounding_box] queries"),
        "{other}"
    );
    let missing = all(r#"{"geo_bounding_box":{"nope":{"top":1,"left":0,"bottom":0,"right":1}}}"#)
        .expect_err("an unmapped field");
    assert!(
        missing.contains("failed to find geo field [nope]"),
        "{missing}"
    );
    let none = all(
        r#"{"geo_bounding_box":{"ignore_unmapped":true,"nope":{"top":1,"left":0,"bottom":0,"right":1}}}"#,
    )
    .expect("ignored");
    assert!(none.is_empty());
}
