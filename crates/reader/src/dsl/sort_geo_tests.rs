//! Tests of the `_geo_distance` sort parser; compiled only under
//! `#[cfg(test)]`.

use lucene_search::document::geo::DistanceMode;

use super::sort::SortKey;
use super::sort_geo::GeoDistanceSort;
use crate::dsl::parse_chunks;

fn parsed(inner: &str) -> GeoDistanceSort {
    let body = format!(r#"{{"sort":[{{"_geo_distance":{{{inner}}}}}]}}"#);
    match parse_chunks([body.as_bytes()]).map(|s| s.sort.into_iter().next()) {
        Ok(Some(SortKey::GeoDistance(g))) => *g,
        other => panic!("parsed {other:?}"),
    }
}

fn reason(inner: &str) -> String {
    let body = format!(r#"{{"sort":[{{"_geo_distance":{{{inner}}}}}]}}"#);
    match parse_chunks([body.as_bytes()]) {
        Err(e) => e.into_failure().reason,
        Ok(s) => panic!("parsed {s:?}"),
    }
}

#[test]
fn every_origin_form_parses() {
    for inner in [
        r#""loc":{"lat":1.5,"lon":2.5}"#,
        r#""loc":[2.5,1.5]"#,
        r#""loc":"1.5,2.5""#,
        r#""loc":[{"lat":1.5,"lon":2.5}]"#,
        r#""loc":[[2.5,1.5]]"#,
        r#""loc":["1.5,2.5"]"#,
    ] {
        let g = parsed(inner);
        assert_eq!(
            (g.field.as_str(), g.points.clone()),
            ("loc", vec![(1.5, 2.5)]),
            "{inner}"
        );
        assert!(g.ascending);
        assert_eq!(g.mode, DistanceMode::Min);
        assert_eq!(g.unit_meters, 1.0);
        assert!(!g.plane);
    }
    let g = parsed(r#""loc":[[2.5,1.5],{"lat":3,"lon":4}]"#);
    assert_eq!(g.points, vec![(1.5, 2.5), (3.0, 4.0)]);
}

#[test]
fn options_parse_and_the_mode_follows_the_order() {
    let g = parsed(
        r#""loc":[0,0],"order":"desc","unit":"km","distance_type":"plane","validation_method":"coerce","ignore_unmapped":true"#,
    );
    assert!(!g.ascending);
    assert_eq!(g.mode, DistanceMode::Max);
    assert_eq!(g.unit_meters, 1000.0);
    assert!(g.plane && g.ignore_unmapped);
    for (text, mode) in [
        ("min", DistanceMode::Min),
        ("MAX", DistanceMode::Max),
        ("sum", DistanceMode::Sum),
        ("avg", DistanceMode::Avg),
        ("median", DistanceMode::Median),
    ] {
        assert_eq!(
            parsed(&format!(r#""loc":[0,0],"mode":"{text}""#)).mode,
            mode
        );
        assert_eq!(
            parsed(&format!(r#""loc":[0,0],"sort_mode":"{text}""#)).mode,
            mode
        );
    }
}

#[test]
fn bad_keys_are_refused_with_the_javas_messages() {
    assert_eq!(
        reason(r#""order":"asc""#),
        "Geo distance sorting needs at least one point."
    );
    assert_eq!(reason(r#""loc":[0,0],"mode":"x""#), "Unknown SortMode [x]");
    assert_eq!(
        reason(r#""loc":[0,0],"distance_type":"x""#),
        "No geo distance for [x]"
    );
    assert_eq!(
        reason(r#""loc":[0,0],"unit":"parsec""#),
        "No distance unit match [parsec]"
    );
    assert!(
        reason(r#""a":"1,2","b":"3,4""#)
            .contains("Trying to reset fieldName to [b], already set to [a].")
    );
    assert!(
        reason(r#""a":{"lat":1,"lon":2},"b":{"lat":1,"lon":2}"#)
            .contains("Trying to reset fieldName to [b]")
    );
    assert!(
        reason(r#""loc":[0,0],"boost":2"#).contains("[_geo_distance] does not support [boost]")
    );
    assert!(
        reason(r#""loc":[0,"x"]"#)
            .contains("geo point parsing: expected second number but got [VALUE_STRING] instead")
    );
    assert!(
        reason(r#""loc":[0,0],"nested":{"path":"a"}"#).contains("not supported by this node yet")
    );
    assert!(reason(r#""loc":[0,0],"nested_path":"a""#).contains("not supported by this node yet"));
    assert!(
        reason(r#""loc":[0,0],"nested_filter":{"match_all":{}}"#)
            .contains("not supported by this node yet")
    );
}
