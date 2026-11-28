//! Tests of the `geo_distance` parser; compiled only under `#[cfg(test)]`.

use super::geo_distance::{GeoDistance, Validation};
use super::*;
use crate::dsl::parse_chunks;
use crate::failure::Failure;

fn parsed(body: &str) -> GeoDistance {
    match parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => match *m {
            More::GeoDistance(g) => *g,
            other => panic!("parsed {other:?}"),
        },
        other => panic!("parsed {other:?}"),
    }
}

fn failure(body: &str) -> Failure {
    match parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]) {
        Err(e) => e.into_failure(),
        Ok(s) => panic!("parsed {s:?}"),
    }
}

fn reason(inner: &str) -> String {
    failure(&format!(r#"{{"geo_distance":{{{inner}}}}}"#)).reason
}

#[test]
fn every_centre_form_parses_to_the_same_point() {
    for centre in [
        r#""loc":{"lat":40.5,"lon":-70.25}"#,
        r#""loc":[-70.25,40.5]"#,
        r#""loc":"40.5,-70.25""#,
        r#""loc.lat":40.5,"loc.lon":-70.25"#,
    ] {
        let g = parsed(&format!(
            r#"{{"geo_distance":{{"distance":"2km",{centre},"boost":3}}}}"#
        ));
        assert_eq!(
            (g.field.as_str(), g.lat, g.lon, g.meters, g.boost),
            ("loc", 40.5, -70.25, 2000.0, 3.0),
            "{centre}"
        );
        assert_eq!(g.validation, Validation::Strict);
        assert!(!g.ignore_unmapped);
    }
}

#[test]
fn a_geohash_centre_is_the_cells_lower_left_corner() {
    let g = parsed(r#"{"geo_distance":{"distance":"1km","loc":{"geohash":"u4pruydqqvj"}}}"#);
    assert!((g.lat - 57.64911).abs() < 1e-4 && (g.lon - 10.40744).abs() < 1e-4);
}

#[test]
fn distances_take_units_and_a_default_unit() {
    let m = |inner: &str| parsed(&format!(r#"{{"geo_distance":{{{inner},"loc":[0,0]}}}}"#)).meters;
    assert_eq!(m(r#""distance":"1mi""#), 1609.344);
    assert_eq!(m(r#""distance":"3ft""#), 3.0 * 0.3048);
    assert_eq!(m(r#""distance":"2.5""#), 2.5);
    assert_eq!(m(r#""distance":7"#), 7.0);
    assert_eq!(m(r#""distance":2,"unit":"km""#), 2000.0);
    assert_eq!(m(r#""distance":1.5e3"#), 1500.0);
}

#[test]
fn options_parse() {
    let g = parsed(
        r#"{"geo_distance":{"distance":"1m","validation_method":"coerce","ignore_unmapped":true,"distance_type":"plane","loc":[1,2]}}"#,
    );
    assert_eq!(g.validation, Validation::Coerce);
    assert!(g.ignore_unmapped);
    let g = parsed(
        r#"{"geo_distance":{"distance":"1m","validation_method":"IGNORE_MALFORMED","loc":[1,2]}}"#,
    );
    assert_eq!(g.validation, Validation::IgnoreMalformed);
}

#[test]
fn bad_parameters_are_refused_with_the_javas_messages() {
    assert_eq!(
        reason(r#""loc":[1,2]"#),
        "geo_distance requires 'distance' to be specified"
    );
    assert_eq!(
        reason(r#""distance":"1km""#),
        "fieldName must not be null or empty"
    );
    assert_eq!(
        reason(r#""distance":"0km","loc":[1,2]"#),
        "distance must be greater than zero"
    );
    assert_eq!(
        reason(r#""distance":"","loc":[1,2]"#),
        "distance must not be null or empty"
    );
    assert_eq!(
        reason(r#""distance":"1km","distance_type":"x","loc":[1,2]"#),
        "No geo distance for [x]"
    );
    assert_eq!(
        reason(r#""distance":"1km","unit":"parsec","loc":[1,2]"#),
        "No distance unit match [parsec]"
    );
    assert_eq!(
        reason(r#""distance":"abc","loc":[1,2]"#),
        "For input string: \"abc\""
    );
    assert_eq!(
        reason(r#""distance":"1km","validation_method":"x","loc":[1,2]"#),
        failure(r#"{"geo_distance":{"distance":"1km","validation_method":"x","loc":[1,2]}}"#)
            .reason
    );
}

#[test]
fn bad_centres_are_refused() {
    assert!(reason(r#""distance":"1km","loc":[1]"#).contains("at least two dimensions"));
    assert!(reason(r#""distance":"1km","loc":["a",2]"#).contains("numeric value expected"));
    assert!(reason(r#""distance":"1km","loc":[1,2,3]"#).contains("found Z value [3"));
    assert!(reason(r#""distance":"1km","loc":{"x":1}"#).contains("does not support [loc]"));
    assert!(
        reason(r#""distance":"1km","a":{"lat":1,"lon":2},"b":{"lat":1,"lon":2}"#)
            .contains("doesn't support multiple fields, found [a] and [b]")
    );
    assert!(reason(r#""distance":"1km","a":"1,2","b":"3,4""#).contains("unexpected field [b]"));
    assert!(reason(r#""distance":"1km","loc":{"lat":"x","lon":2}"#).contains("x"));
    assert!(reason(r#""distance":"1km","loc":{"lat":true,"lon":2}"#).contains("true"));
}
