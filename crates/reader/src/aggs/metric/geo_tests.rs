use super::*;

fn centroid(count: u64, lat: f64, lon: f64) -> Value {
    Value::GeoCentroid { count, lat, lon }
}

fn bounds(extremes: [f64; 6], wrap: bool) -> Value {
    let [top, bottom, pos_left, pos_right, neg_left, neg_right] = extremes;
    Value::GeoBounds {
        top,
        bottom,
        pos_left,
        pos_right,
        neg_left,
        neg_right,
        wrap,
    }
}

// ALLOC: one small body in a test.
fn body(value: &Value) -> String {
    let mut out = Vec::new();
    match *value {
        Value::GeoCentroid { count, lat, lon } => write_centroid(&mut out, count, lat, lon),
        Value::GeoBounds {
            top,
            bottom,
            pos_left,
            pos_right,
            neg_left,
            neg_right,
            wrap,
        } => write_bounds(
            &mut out, top, bottom, pos_left, pos_right, neg_left, neg_right, wrap,
        ),
        _ => unreachable!(),
    }
    .unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn a_centroid_reduce_weights_each_shard_by_its_count_and_skips_empty_ones() {
    let parts = [
        centroid(2, 10.0, 20.0),
        centroid(0, 0.0, 0.0),
        centroid(1, 40.0, 50.0),
    ];
    assert_eq!(reduce_centroid(&parts), Some(centroid(3, 20.0, 30.0)));
}

#[test]
fn a_centroid_reduce_of_empty_shards_is_empty() {
    let parts = [centroid(0, 0.0, 0.0), centroid(0, 0.0, 0.0)];
    assert_eq!(reduce_centroid(&parts), Some(centroid(0, 0.0, 0.0)));
}

#[test]
fn a_reduce_of_another_kind_is_refused() {
    assert_eq!(reduce_centroid(&[Value::Min(1.0)]), None);
    assert_eq!(reduce_bounds(&[centroid(1, 1.0, 1.0)]), None);
}

#[test]
fn a_centroid_body_names_the_location_only_with_a_count() {
    assert_eq!(
        body(&centroid(3, 20.0, 30.0)),
        r#""location":{"lat":20.0,"lon":30.0},"count":3"#
    );
    assert_eq!(body(&centroid(0, 0.0, 0.0)), r#""count":0"#);
}

#[test]
fn a_bounds_reduce_takes_the_extremes_of_the_extremes() {
    let inf = f64::INFINITY;
    // `negLeft` starts at positive infinity and `negRight` at negative.
    let parts = [
        bounds([10.0, 1.0, 5.0, 20.0, inf, -inf], true),
        bounds([40.0, -5.0, 2.0, 8.0, -30.0, -10.0], true),
    ];
    assert_eq!(
        reduce_bounds(&parts),
        Some(bounds([40.0, -5.0, 2.0, 20.0, -30.0, -10.0], true))
    );
}

#[test]
fn a_bounds_body_is_empty_without_a_point() {
    let inf = f64::INFINITY;
    assert_eq!(body(&bounds([-inf, inf, inf, -inf, inf, -inf], true)), "");
}

#[test]
fn points_on_one_side_of_the_meridian_use_that_side() {
    let inf = f64::INFINITY;
    assert_eq!(
        body(&bounds([40.0, -5.0, 2.0, 20.0, inf, -inf], true)),
        r#""bounds":{"top_left":{"lat":40.0,"lon":2.0},"bottom_right":{"lat":-5.0,"lon":20.0}}"#
    );
    assert_eq!(
        body(&bounds([40.0, -5.0, inf, -inf, -30.0, -10.0], true)),
        r#""bounds":{"top_left":{"lat":40.0,"lon":-30.0},"bottom_right":{"lat":-5.0,"lon":-10.0}}"#
    );
}

#[test]
fn points_either_side_wrap_when_the_wrapped_box_is_narrower() {
    // Unwrapped width 350, wrapped width 20.
    let extremes = [10.0, -10.0, 170.0, 175.0, -175.0, -170.0];
    assert_eq!(
        body(&bounds(extremes, true)),
        r#""bounds":{"top_left":{"lat":10.0,"lon":170.0},"bottom_right":{"lat":-10.0,"lon":-170.0}}"#
    );
    assert_eq!(
        body(&bounds(extremes, false)),
        r#""bounds":{"top_left":{"lat":10.0,"lon":-175.0},"bottom_right":{"lat":-10.0,"lon":175.0}}"#
    );
}

fn parsed(body: &str) -> crate::dsl::SearchSource {
    crate::dsl::parse_chunks([body.as_bytes()]).expect("parses")
}

fn refusal(body: &str) -> String {
    crate::dsl::parse_chunks([body.as_bytes()])
        .expect_err("refused")
        .into_failure()
        .reason
        .clone()
}

#[test]
fn a_geo_body_carries_its_field_and_wrap_longitude() {
    let s = parsed(
        r#"{"aggs":{"c":{"geo_centroid":{"field":"p"}},"b":{"geo_bounds":{"field":"q"}},"u":{"geo_bounds":{"field":"r","wrap_longitude":false}},"s":{"geo_bounds":{"field":"t","wrap_longitude":"false"}}}}"#,
    );
    let got: Vec<(&str, &str, bool)> = s
        .aggs
        .iter()
        .map(|a| (a.name.as_str(), a.field.as_str(), a.wrap_longitude))
        .collect(); // ALLOC: four entries in a test.
    assert_eq!(
        got,
        [
            ("c", "p", true),
            ("b", "q", true),
            ("u", "r", false),
            ("s", "t", false)
        ]
    );
}

#[test]
fn a_geo_body_with_no_field_names_the_fields_its_builder_declares() {
    assert_eq!(
        refusal(r#"{"aggs":{"a":{"geo_bounds":{}}}}"#),
        "Required one of fields [field], but none were specified. "
    );
    assert_eq!(
        refusal(r#"{"aggs":{"a":{"geo_centroid":{}}}}"#),
        "Required one of fields [field, script], but none were specified. "
    );
}
