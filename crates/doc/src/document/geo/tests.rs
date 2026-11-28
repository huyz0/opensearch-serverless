use super::*;

fn num(t: &str) -> Json {
    Json::Num(t.to_owned())
}

fn chain(e: &MappingError) -> Vec<(&str, &str)> {
    let mut out = vec![(e.ty, e.reason.as_str())];
    let mut at = e;
    while let Some(c) = &at.caused_by {
        out.push((c.ty, c.reason.as_str()));
        at = c;
    }
    out
}

#[test]
fn a_refused_text_carries_the_chain_stock_reports() {
    let e = string_point("DRM3", false).expect_err("refused");
    assert_eq!(
        chain(&e),
        [
            (
                "parse_exception",
                "unsupported symbol [D] in geohash [DRM3]"
            ),
            (
                "illegal_argument_exception",
                "unsupported symbol [D] in geohash [DRM3]"
            )
        ]
    );
    let e = string_point("not a point", false).expect_err("refused");
    assert_eq!(
        chain(&e),
        [
            ("parse_exception", "Invalid WKT format"),
            ("illegal_argument_exception", "Unknown geometry type: not")
        ]
    );
    let e = string_point("POINT Z (1 2 3)", false).expect_err("refused");
    assert_eq!(
        chain(&e),
        [
            ("parse_exception", "Invalid WKT format"),
            ("java_parse_exception", "expected EMPTY or ( but found: Z")
        ]
    );
    let e = string_point("POINT (1)", false).expect_err("refused");
    assert_eq!(
        chain(&e),
        [
            ("parse_exception", "Invalid WKT format"),
            ("java_parse_exception", "expected number but found: ')'")
        ]
    );
}

#[test]
fn a_geohash_decodes_to_its_lower_left_corner() {
    let (lat, lon) = geohash("u4pruydqqvj").expect("hash");
    assert!((lat - 57.64911).abs() < 1e-4 && (lon - 10.40744).abs() < 1e-4);
    assert!(geohash("").is_err() && geohash("a").is_err());
}

#[test]
fn every_form_reads_as_the_same_point() {
    let want = (41.12, -71.34);
    let obj = Json::Obj(vec![
        ("lat".into(), num("41.12")),
        ("lon".into(), num("-71.34")),
    ]);
    let s = Json::Str("41.12,-71.34".into());
    let arr = Json::Arr(vec![num("-71.34"), num("41.12")]);
    let wkt = Json::Str("POINT (-71.34 41.12)".into());
    let gj = Json::Obj(vec![
        ("type".into(), Json::Str("Point".into())),
        ("coordinates".into(), arr.clone()),
    ]);
    for j in [obj, s, arr, wkt, gj] {
        assert_eq!(parse_point(&j, true).expect("point"), want);
    }
}

#[test]
fn a_point_out_of_range_is_refused_or_normalized() {
    let (lat, lon) = process("p", (91.0, 0.0), true).expect("normalized");
    assert_eq!((lat, lon), (89.0, 180.0));
    let e = process("p", (91.0, 0.0), false).expect_err("refused");
    assert_eq!(e.reason, "illegal latitude value [91.0] for p");
    assert!(process("p", (f64::NAN, 0.0), true).is_err());
}
