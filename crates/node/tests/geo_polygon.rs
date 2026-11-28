//! docs/specs/geospatial AC-2 (the `geo_polygon` part): the ids a
//! `geo_polygon` query returns over a fixed corpus of 1,000 points, the
//! dateline and the poles among them, equal those of a brute-force point-in-polygon test
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

/// Even-odd ray casting in the plane of degrees, as the query's polygon is.
fn inside(shell: &[(f64, f64)], p: (f64, f64)) -> bool {
    let mut inside = false;
    for w in shell.windows(2) {
        let ((ay, ax), (by, bx)) = (w[0], w[1]);
        if (ay > p.0) != (by > p.0) && p.1 < (bx - ax) * (p.0 - ay) / (by - ay) + ax {
            inside = !inside;
        }
    }
    inside
}

/// The distance in degrees from a point to the nearest edge.
fn edge_distance(shell: &[(f64, f64)], p: (f64, f64)) -> f64 {
    shell
        .windows(2)
        .map(|w| {
            let ((ay, ax), (by, bx)) = (w[0], w[1]);
            let (dy, dx) = (by - ay, bx - ax);
            let len2 = dy * dy + dx * dx;
            let t = if len2 == 0.0 {
                0.0
            } else {
                (((p.0 - ay) * dy + (p.1 - ax) * dx) / len2).clamp(0.0, 1.0)
            };
            ((p.0 - (ay + t * dy)).powi(2) + (p.1 - (ax + t * dx)).powi(2)).sqrt()
        })
        .fold(f64::MAX, f64::min)
}

fn points_json(shell: &[(f64, f64)]) -> String {
    let items: Vec<String> = shell
        .iter()
        .map(|(lat, lon)| format!(r#"{{"lat":{lat},"lon":{lon}}}"#))
        .collect();
    format!("[{}]", items.join(","))
}

// docs/specs/geospatial AC-2
#[test]
fn geo_polygon_returns_the_oracles_ids() {
    let cluster = cluster();
    let points = corpus();
    // A triangle, a concave L, a polygon touching both poles' latitudes and the
    // dateline's longitudes, a sliver, and an unclosed shell.
    let polygons: [Vec<(f64, f64)>; 5] = [
        vec![(0.0, 0.0), (0.0, 60.0), (50.0, 30.0), (0.0, 0.0)],
        vec![
            (0.0, -90.0),
            (0.0, 0.0),
            (30.0, 0.0),
            (30.0, -30.0),
            (60.0, -30.0),
            (60.0, -90.0),
            (0.0, -90.0),
        ],
        vec![
            (-90.0, -180.0),
            (-90.0, 180.0),
            (90.0, 180.0),
            (90.0, -180.0),
            (-90.0, -180.0),
        ],
        vec![(10.0, 10.0), (10.5, 80.0), (11.0, 10.0), (10.0, 10.0)],
        vec![(-40.0, 100.0), (-40.0, 179.0), (20.0, 179.0), (20.0, 100.0)],
    ];
    let mut compared = 0usize;
    for shell in &polygons {
        let mut closed = shell.clone();
        if closed.first() != closed.last() {
            closed.push(closed[0]);
        }
        let q = format!(
            r#"{{"geo_polygon":{{"loc":{{"points":{}}}}}}}"#,
            points_json(shell)
        );
        let mut want = Vec::new();
        let mut edge = Vec::new();
        for (i, p) in points.iter().enumerate() {
            if edge_distance(&closed, *p) < 1e-4 {
                edge.push(i);
            } else if inside(&closed, *p) {
                want.push(i);
            }
        }
        let got: Vec<usize> = wait_for_ids(&cluster, &q)
            .into_iter()
            .filter(|i| !edge.contains(i))
            .collect();
        assert_eq!(got, want, "{q}");
        compared += want.len();
    }
    assert!(compared > 500, "the cases match something: {compared}");
}

// docs/specs/geospatial AC-2
#[test]
fn geo_polygon_refuses_bad_input_and_honours_coerce_and_ignore_unmapped() {
    let cluster = cluster();
    let all = |q: &str| search(&cluster, q);
    let strict = all(r#"{"geo_polygon":{"loc":{"points":["91,0","0,10","10,10"]}}}"#)
        .expect_err("a latitude past 90 is refused");
    assert!(
        strict.contains("illegal latitude value [91.0] for [geo_polygon]"),
        "{strict}"
    );
    let lon = all(r#"{"geo_polygon":{"loc":{"points":["0,181","0,10","10,10"]}}}"#)
        .expect_err("a longitude past 180 is refused");
    assert!(
        lon.contains("illegal longitude value [181.0] for [geo_polygon]"),
        "{lon}"
    );
    all(r#"{"geo_polygon":{"validation_method":"COERCE","loc":{"points":["0,181","0,10","10,10"]}}}"#)
        .expect("coerce wraps the longitude");
    let other = all(r#"{"geo_polygon":{"other":{"points":["0,0","0,10","10,10"]}}}"#)
        .expect_err("a keyword field is not a geo field");
    assert!(
        other.contains("field [other] is not a geo_point field"),
        "{other}"
    );
    let missing = all(r#"{"geo_polygon":{"nope":{"points":["0,0","0,10","10,10"]}}}"#)
        .expect_err("an unmapped field");
    assert!(
        missing.contains("failed to find geo_point field [nope]"),
        "{missing}"
    );
    let none =
        all(r#"{"geo_polygon":{"ignore_unmapped":true,"nope":{"points":["0,0","0,10","10,10"]}}}"#)
            .expect("ignored");
    assert!(none.is_empty());
}
