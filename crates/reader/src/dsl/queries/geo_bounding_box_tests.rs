//! Tests of the `geo_bounding_box` parser; compiled only under
//! `#[cfg(test)]`.

use super::geo_bounding_box::GeoBoundingBox;
use super::geo_distance::Validation;
use super::*;
use crate::dsl::parse_chunks;
use crate::failure::Failure;

fn parsed(body: &str) -> GeoBoundingBox {
    match parse_chunks([format!(r#"{{"query":{body}}}"#).as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => match *m {
            More::GeoBoundingBox(g) => *g,
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
    failure(&format!(r#"{{"geo_bounding_box":{{{inner}}}}}"#)).reason
}

fn corners(inner: &str) -> (f64, f64, f64, f64) {
    let g = parsed(&format!(r#"{{"geo_bounding_box":{{{inner}}}}}"#));
    assert_eq!(g.field, "loc");
    (g.top, g.left, g.bottom, g.right)
}

#[test]
fn every_corner_form_parses_to_the_same_box() {
    let want = (40.5, -70.0, 10.0, 20.25);
    for inner in [
        r#""loc":{"top_left":{"lat":40.5,"lon":-70},"bottom_right":{"lat":10,"lon":20.25}}"#,
        r#""loc":{"top_left":[-70,40.5],"bottom_right":[20.25,10]}"#,
        r#""loc":{"top_left":"40.5,-70","bottom_right":"10,20.25"}"#,
        r#""loc":{"top_left":"POINT (-70 40.5)","bottom_right":"POINT (20.25 10)"}"#,
        r#""loc":{"top":40.5,"left":-70,"bottom":10,"right":20.25}"#,
        r#""loc":{"top_right":[20.25,40.5],"bottom_left":[-70,10]}"#,
        r#""loc":{"top_left":{"type":"Point","coordinates":[-70,40.5]},"bottom_right":{"coordinates":[20.25,10],"type":"point"}}"#,
        r#""loc":{"wkt":"BBOX (-70, 20.25, 40.5, 10)"}"#,
    ] {
        assert_eq!(corners(inner), want, "{inner}");
    }
}

#[test]
fn geohash_corners_are_the_cells_named_corner() {
    // "u" is the 5-bit cell 0 to 45 degrees north, 0 to 45 east.
    let (top, left, bottom, right) =
        corners(r#""loc":{"top_left":"u","bottom_right":{"geohash":"u"}}"#);
    assert_eq!(left, 0.0);
    assert_eq!(bottom, 45.0);
    assert!(top > 0.0 && right > left);
    let (t, l, b, r) = corners(r#""loc":{"top_left":"u","bottom_right":"u"}"#);
    assert!(t > b && r > l, "{t} {l} {b} {r}");
}

#[test]
fn options_parse() {
    let g = parsed(
        r#"{"geo_bounding_box":{"validation_method":"coerce","ignore_unmapped":true,"type":"indexed","boost":2,"loc":{"top":2,"left":0,"bottom":1,"right":1}}}"#,
    );
    assert_eq!(g.validation, Validation::Coerce);
    assert!(g.ignore_unmapped);
    assert_eq!(g.boost, 2.0);
    let g = parsed(r#"{"geo_bounding_box":{"loc":{"top":2,"left":0,"bottom":1,"right":1}}}"#);
    assert_eq!(g.validation, Validation::Strict);
}

#[test]
fn bad_boxes_are_refused_with_the_javas_messages() {
    assert_eq!(
        reason(r#""loc":{"top":0,"left":0,"bottom":10,"right":10}"#),
        "top is below bottom corner: 0.0 vs. 10.0"
    );
    assert_eq!(
        reason(r#""loc":{"top":5,"left":0,"bottom":5,"right":10}"#),
        "top cannot be the same as bottom: 5.0 == 5.0"
    );
    assert_eq!(
        reason(r#""loc":{"top":6,"left":3,"bottom":5,"right":3}"#),
        "left cannot be the same as right: 3.0 == 3.0"
    );
    assert_eq!(
        reason(r#""loc":{"top_left":[1,2]}"#),
        "bottom latitude is invalid: NaN"
    );
    assert_eq!(
        reason(r#""type":"memory""#),
        "failed to parse [geo_bounding_box] query. bounding box not provided"
    );
    assert_eq!(
        reason(r#""type":"x","loc":{"top":6,"left":3,"bottom":5,"right":4}"#),
        "no type can be parsed from ordinal x"
    );
    assert_eq!(
        reason(r#""nope":1,"loc":{"top":6,"left":3,"bottom":5,"right":4}"#),
        "failed to parse [geo_bounding_box] query. unexpected field [nope]"
    );
    assert_eq!(
        reason(r#""validation_method":"x","loc":{"top":6,"left":3,"bottom":5,"right":4}"#),
        "operator needs to be either [COERCE, IGNORE_MALFORMED, STRICT], but not [x]"
    );
}

#[test]
fn bad_box_bodies_are_wrapped_as_the_javas_are() {
    let wrapped = |inner: &str, tail: &str| {
        assert_eq!(
            reason(&format!(r#""loc":{{{inner}}}"#)),
            format!("failed to parse [geo_bounding_box] query. [{tail}]"),
            "{inner}"
        );
    };
    wrapped(
        r#""middle":[1,2]"#,
        "failed to parse bounding box. unexpected field [middle]",
    );
    wrapped(
        r#""wkt":"BBOX (1, 2, 3, 4)","top":1"#,
        "failed to parse bounding box. Conflicting definition found using well-known text and explicit corners.",
    );
    wrapped(r#""wkt":"nonsense""#, "failed to parse WKT bounding box");
    wrapped(
        r#""wkt":"BBOX (1, 2, 3)""#,
        "failed to parse WKT bounding box",
    );
    wrapped(
        r#""wkt":"BBOX (1, 2, 4, 5)""#,
        "failed to parse WKT bounding box",
    );
    wrapped(r#""top_left":5"#, "geo_point expected");
    wrapped(r#""top_left":{"foo":1}"#, super_invalid());
    wrapped(r#""top_left":{"lat":1}"#, "field [lon] missing");
    wrapped(r#""top_left":{"lon":1}"#, "field [lat] missing");
    wrapped(r#""top_left":{"lat":true,"lon":1}"#, "lat must be a number");
    wrapped(
        r#""top_left":{"lat":"x","lon":1}"#,
        "[lon] and [lat] must be valid double values",
    );
    wrapped(r#""top_left":{"geohash":5}"#, "geohash must be a string");
    wrapped(
        r#""top_left":{"type":"Line","coordinates":[1,2]}"#,
        "type must be Point",
    );
    wrapped(
        r#""top_left":{"type":"Point"}"#,
        "field [coordinates] missing",
    );
    wrapped(r#""top_left":{"lat":1,"lon":2,"x":3}"#, super_invalid());
    wrapped(r#""top_left":{}"#, "token [END_OBJECT] not allowed");
    wrapped(
        r#""top_left":"aaa""#,
        "unsupported symbol [a] in geohash [aaa]",
    );
}

fn super_invalid() -> &'static str {
    "field must be either [lon|lat], [type|coordinates], or [geohash]"
}
