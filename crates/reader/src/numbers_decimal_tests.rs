//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::*;

fn num(s: &str) -> Value {
    Value::Number(s.into())
}

fn st(s: &str) -> Value {
    Value::Str(s.into())
}

fn reason<T: std::fmt::Debug>(r: Result<T, Failure>) -> String {
    match r {
        Err(f) => f.reason,
        Ok(v) => panic!("parsed {v:?}"),
    }
}

#[test]
fn a_double_parses_as_parse_double_and_must_be_finite() {
    assert_eq!(parse_double(&num("1.5")).expect("ok"), 1.5);
    assert_eq!(parse_double(&st(" 2e2 ")).expect("ok"), 200.0);
    assert_eq!(reason(parse_double(&st("x"))), "For input string: \"x\"");
    assert_eq!(
        reason(parse_double(&st("Infinity"))),
        "[double] supports only finite values, but got [Infinity]"
    );
    assert_eq!(
        reason(parse_double(&st("NaN"))),
        "[double] supports only finite values, but got [NaN]"
    );
}

#[test]
fn a_float_rounds_to_the_nearest_float_and_must_be_finite() {
    assert_eq!(parse_float(&num("0.1")).expect("ok"), 0.1_f32);
    assert_eq!(
        reason(parse_float(&num("1e39"))),
        "[float] supports only finite values, but got [Infinity]"
    );
}

#[test]
fn an_excluded_bound_steps_one_double_in() {
    let (l, u) = double_range(Some(&num("1")), Some(&num("2")), false, false)
        .expect("ok")
        .expect("bounds");
    assert_eq!(l, 1.0_f64.next_up());
    assert_eq!(u, 2.0_f64.next_down());
    let (l, u) = double_range(Some(&num("1")), None, true, true)
        .expect("ok")
        .expect("bounds");
    assert_eq!((l, u), (1.0, f64::INFINITY));
    // An empty interval matches nothing.
    assert_eq!(
        double_range(Some(&num("1")), Some(&num("1")), false, true).expect("ok"),
        None
    );
}

#[test]
fn zero_steps_across_the_sign_as_lucenes_next_up_and_down_do() {
    assert_eq!(double_up(-0.0).to_bits(), 0.0_f64.to_bits());
    assert_eq!(double_down(0.0).to_bits(), (-0.0_f64).to_bits());
    assert_eq!(float_up(-0.0).to_bits(), 0.0_f32.to_bits());
    assert_eq!(float_down(0.0).to_bits(), (-0.0_f32).to_bits());
    assert_eq!(double_up(1.0), 1.0_f64.next_up());
    assert_eq!(double_down(1.0), 1.0_f64.next_down());
    assert_eq!(float_up(1.0), 1.0_f32.next_up());
    assert_eq!(float_down(1.0), 1.0_f32.next_down());
}

#[test]
fn a_float_range_steps_one_float_in() {
    let (l, u) = float_range(Some(&num("0.5")), Some(&num("1.5")), false, false)
        .expect("ok")
        .expect("bounds");
    assert_eq!(l, 0.5_f32.next_up());
    assert_eq!(u, 1.5_f32.next_down());
    assert_eq!(
        float_range(Some(&num("1")), Some(&num("1")), true, false).expect("ok"),
        None
    );
}

#[test]
fn points_encode_as_lucene_sorts_them() {
    let q = double_points("d", -1.5, 2.5).expect("query");
    assert_eq!((q.num_dims, q.bytes_per_dim), (1, 8));
    assert!(q.lower < q.upper);
    let q = float_points("f", -1.5, 2.5).expect("query");
    assert_eq!(q.bytes_per_dim, 4);
    assert!(q.lower < q.upper);
    assert_eq!(
        double_set("d", &[2.0, 1.0, 2.0]).expect("set").points.len(),
        2
    );
    assert_eq!(float_set("f", &[2.0, 1.0]).expect("set").points.len(), 2);
}
