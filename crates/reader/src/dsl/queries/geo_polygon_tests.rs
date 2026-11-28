//! Tests of the `geo_polygon` parser; compiled only under `#[cfg(test)]`.

use super::geo_distance::Validation;
use super::geo_polygon::GeoPolygon;
use super::*;
use crate::dsl::parse_chunks;

fn parsed(inner: &str) -> GeoPolygon {
    let body = format!(r#"{{"query":{{"geo_polygon":{{{inner}}}}}}}"#);
    match parse_chunks([body.as_bytes()]).map(|s| s.query) {
        Ok(Some(Query::More(m))) => match *m {
            More::GeoPolygon(g) => *g,
            other => panic!("parsed {other:?}"),
        },
        other => panic!("parsed {other:?}"),
    }
}

fn reason(inner: &str) -> String {
    let body = format!(r#"{{"query":{{"geo_polygon":{{{inner}}}}}}}"#);
    match parse_chunks([body.as_bytes()]) {
        Err(e) => e.into_failure().reason,
        Ok(s) => panic!("parsed {s:?}"),
    }
}

#[test]
fn points_in_every_form_parse_and_the_shell_closes() {
    let g = parsed(
        r#""loc":{"points":[{"lat":0,"lon":0},[10,0],"10,10",{"type":"Point","coordinates":[0,10]}]},"boost":2,"ignore_unmapped":true,"validation_method":"coerce""#,
    );
    assert_eq!(g.field, "loc");
    assert_eq!(
        g.shell,
        vec![
            (0.0, 0.0),
            (0.0, 10.0),
            (10.0, 10.0),
            (10.0, 0.0),
            (0.0, 0.0)
        ]
    );
    assert_eq!((g.boost, g.ignore_unmapped), (2.0, true));
    assert_eq!(g.validation, Validation::Coerce);
    let closed = parsed(r#""loc":{"points":["0,0","0,10","10,10","0,0"]}"#);
    assert_eq!(closed.shell.len(), 4);
    assert_eq!(closed.validation, Validation::Strict);
}

#[test]
fn bad_polygons_are_refused_with_the_javas_messages() {
    assert_eq!(
        reason(r#""loc":{"points":["0,0","0,10"]}"#),
        "too few points defined for geo_polygon query"
    );
    assert_eq!(
        reason(r#""loc":{"points":["0,0","0,10","1,1","0,0"]},"x":1"#),
        "[geo_polygon] query does not support [x]"
    );
    assert_eq!(
        reason(r#""loc":{"points":[]}"#),
        "polygon must not be null or empty"
    );
    assert_eq!(reason(r#""boost":1"#), "fieldName must not be null");
    assert_eq!(
        reason(r#""loc":{"nope":["0,0"]}"#),
        "[geo_polygon] query does not support [nope]"
    );
    assert!(
        reason(r#""loc":{"points":"x"}"#).contains(
            "[geo_polygon] query does not support token type [VALUE_STRING] under [points]"
        )
    );
    assert!(
        reason(r#""loc":{"points":["0,0"]},"validation_method":"x""#)
            .contains("operator needs to be")
    );
    assert!(reason(r#""loc":{"points":[5]}"#).contains("geo_point expected"));
}
