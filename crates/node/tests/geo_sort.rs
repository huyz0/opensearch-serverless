//! docs/specs/geospatial AC-5 (the `_geo_distance` sort): over a fixed corpus
//! of 1,000 points, the dateline and the poles among them, the order and
//! the values of a `_geo_distance` sort follow a haversine oracle written
//! here, in each unit and order, with two origins, and with documents that
//! hold no point at +Infinity. No stock-server recording exists yet.

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

const EARTH: f64 = 6_371_008.771_4;

/// Haversine metres.
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = la2 - la1;
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH * h.sqrt().min(1.0).asin()
}

/// `(id, first sort value)` of every hit, in order.
fn sorted(cluster: &Cluster, sort: &str) -> Vec<(usize, Option<f64>)> {
    let body =
        format!(r#"{{"size":1100,"_source":false,"query":{{"match_all":{{}}}},"sort":[{sort}]}}"#);
    let mut out = Vec::new();
    wait_for("the sorted search answers", 40, || {
        let r = cluster.call(0, "POST", "/places/_search", Some(&body));
        assert_eq!(r.status, 200, "{}", r.text());
        out.clear();
        let text = r.text();
        let mut rest = text.as_str();
        while let Some(at) = rest.find("\"_id\":\"") {
            rest = &rest[at + 7..];
            let end = rest.find('"').expect("closes");
            let id: usize = rest[..end].parse().expect("numeric id");
            let sort_at = rest.find("\"sort\":[").expect("a sort value");
            let tail = &rest[sort_at + 8..];
            let close = tail.find(']').expect("closes");
            let value = tail[..close].split(',').next().expect("one value");
            out.push((id, value.parse::<f64>().ok()));
        }
        true
    });
    out
}

fn origins_distance(origins: &[(f64, f64)], p: (f64, f64), max: bool) -> f64 {
    let mut ds = origins.iter().map(|o| distance(*o, p));
    let first = ds.next().expect("an origin");
    ds.fold(first, |a, d| if max { a.max(d) } else { a.min(d) })
}

/// Hits are in order of the oracle's distance, allowing the sloppy haversine's
/// relative error, and their values are within it too.
fn check(
    got: &[(usize, Option<f64>)],
    points: &[(f64, f64)],
    origins: &[(f64, f64)],
    unit: f64,
    ascending: bool,
    max: bool,
) {
    assert_eq!(got.len(), points.len() + 5, "every document comes back");
    // A document with no point is at +Infinity: last ascending, first descending.
    let (with, without) = if ascending {
        let (a, b) = got.split_at(points.len());
        (a, b)
    } else {
        let (b, a) = got.split_at(5);
        (a, b)
    };
    assert!(
        without.iter().all(|(id, _)| *id >= points.len()),
        "{without:?}"
    );
    let tolerance = 2e-3;
    let mut previous: Option<f64> = None;
    for (id, value) in with {
        assert!(*id < points.len(), "{id}");
        let want = origins_distance(origins, points[*id], max) / unit;
        let value = value.expect("a distance");
        assert!(
            (value - want).abs() <= want.abs() * tolerance + 1e-3,
            "id {id}: sort value {value} against oracle {want}"
        );
        if let Some(p) = previous {
            let slack = p.abs().max(want.abs()) * tolerance + 1e-3;
            assert!(
                if ascending {
                    p <= want + slack
                } else {
                    p + slack >= want
                },
                "id {id} out of order: {p} then {want}"
            );
        }
        previous = Some(want);
    }
}

fn with_missing(cluster: &Cluster) {
    let mut body = String::new();
    for i in 0..5 {
        let id = 1_000 + i;
        body.push_str(&format!(
            "{{\"index\":{{\"_index\":\"places\",\"_id\":\"{id}\"}}}}\n{{\"other\":\"none\"}}\n"
        ));
    }
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
    assert!(r.text().contains("\"errors\":false"), "{}", r.text());
}

/// A sort, its origins, the unit's metres, ascending, and the `max` mode.
type Case<'a> = (&'a str, &'a [(f64, f64)], f64, bool, bool);

// docs/specs/geospatial AC-5
#[test]
fn geo_distance_sort_follows_the_oracle_in_each_unit_order_and_mode() {
    let cluster = cluster();
    with_missing(&cluster);
    let points = corpus();
    let a = (10.0, 20.0);
    let b = (-50.0, 150.0);
    let cases: [Case; 6] = [
        (
            r#"{"_geo_distance":{"loc":[{"lat":10,"lon":20}]}}"#,
            &[a],
            1.0,
            true,
            false,
        ),
        (
            r#"{"_geo_distance":{"loc":[20,10],"unit":"km"}}"#,
            &[a],
            1000.0,
            true,
            false,
        ),
        (
            r#"{"_geo_distance":{"loc":"10,20","unit":"mi","order":"desc"}}"#,
            &[a],
            1609.344,
            false,
            true,
        ),
        (
            r#"{"_geo_distance":{"loc":[{"lat":10,"lon":20},{"lat":-50,"lon":150}],"unit":"km"}}"#,
            &[a, b],
            1000.0,
            true,
            false,
        ),
        (
            r#"{"_geo_distance":{"loc":[{"lat":10,"lon":20},{"lat":-50,"lon":150}],"unit":"km","mode":"max"}}"#,
            &[a, b],
            1000.0,
            true,
            true,
        ),
        (
            r#"{"_geo_distance":{"loc":{"lat":10,"lon":20},"distance_type":"plane","order":"asc"}}"#,
            &[a],
            1.0,
            true,
            false,
        ),
    ];
    for (sort, origins, unit, ascending, max) in cases {
        let got = sorted(&cluster, sort);
        if sort.contains("plane") {
            // The plane distance is not the haversine: the order is checked
            // against its own formula in the engine; here only that every
            // document comes back, those with no point last.
            assert_eq!(got.len(), points.len() + 5);
            assert!(
                got[points.len()..]
                    .iter()
                    .all(|(id, _)| *id >= points.len())
            );
            continue;
        }
        check(&got, &points, origins, unit, ascending, max);
    }
}

// docs/specs/geospatial AC-5
#[test]
fn geo_distance_sort_refuses_bad_input_and_honours_coerce_and_ignore_unmapped() {
    let cluster = cluster();
    let run = |sort: &str| {
        let body = format!(r#"{{"query":{{"match_all":{{}}}},"sort":[{sort}]}}"#);
        let r = cluster.call(0, "POST", "/places/_search", Some(&body));
        (r.status, r.text())
    };
    let (status, text) = run(r#"{"_geo_distance":{"loc":[{"lat":91,"lon":0}]}}"#);
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("illegal latitude value [91.0] for [GeoDistanceSort] for field [loc]."),
        "{text}"
    );
    let (status, text) =
        run(r#"{"_geo_distance":{"validation_method":"COERCE","loc":[{"lat":91,"lon":0}]}}"#);
    assert_eq!(status, 200, "{text}");
    let (status, text) = run(r#"{"_geo_distance":{"nope":[{"lat":1,"lon":0}]}}"#);
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("failed to find mapper for [nope] for geo distance based sort"),
        "{text}"
    );
    let (status, text) =
        run(r#"{"_geo_distance":{"ignore_unmapped":true,"nope":[{"lat":1,"lon":0}]}}"#);
    assert_eq!(status, 200, "{text}");
    let (status, text) = run(r#"{"_geo_distance":{"other":[{"lat":1,"lon":0}]}}"#);
    assert_eq!(status, 400, "{text}");
    let (status, text) = run(r#"{"_geo_distance":{"loc":[{"lat":1,"lon":0}],"mode":"x"}}"#);
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("Unknown SortMode [x]"), "{text}");
    let (status, text) = run(r#"{"_geo_distance":{"order":"asc"}}"#);
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("Geo distance sorting needs at least one point."),
        "{text}"
    );
}

// docs/specs/geospatial AC-5
#[test]
fn geo_distance_sort_pages_with_search_after() {
    let cluster = cluster();
    let sort = r#"{"_geo_distance":{"loc":[{"lat":10,"lon":20}],"unit":"km"}}"#;
    let all = sorted(&cluster, sort);
    let search = |after: Option<f64>, size: usize| {
        let after = after.map_or(String::new(), |v| format!(r#","search_after":[{v:?}]"#));
        let body = format!(
            r#"{{"size":{size},"_source":false,"query":{{"match_all":{{}}}},"sort":[{sort}]{after}}}"#
        );
        let r = cluster.call(0, "POST", "/places/_search", Some(&body));
        assert_eq!(r.status, 200, "{}", r.text());
        let text = r.text();
        let mut ids = Vec::new();
        let mut rest = text.as_str();
        while let Some(at) = rest.find("\"_id\":\"") {
            rest = &rest[at + 7..];
            let end = rest.find('"').expect("closes");
            ids.push(rest[..end].parse::<usize>().expect("numeric id"));
        }
        ids
    };
    let first = search(None, 3);
    assert_eq!(first, all[..3].iter().map(|h| h.0).collect::<Vec<_>>());
    let after = all[2].1.expect("a distance");
    let next = search(Some(after), 3);
    assert_eq!(next, all[3..6].iter().map(|h| h.0).collect::<Vec<_>>());
}
