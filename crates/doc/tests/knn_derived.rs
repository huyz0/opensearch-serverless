//! docs/specs/knn-derived-source: a vector's value in `_source` is replaced
//! by `1` and every other byte is kept.

use oss_doc::lucene::derived::mask_vectors;

fn mask(source: &str, fields: &[&str]) -> Result<String, String> {
    mask_vectors(source.as_bytes(), fields)
        .map(|b| String::from_utf8(b).expect("utf8"))
        .map_err(|e| e.reason)
}

// AC-5
#[test]
fn a_top_level_vector_becomes_one_and_the_rest_is_kept() {
    assert_eq!(
        mask(r#"{"v":[1.0,2.0,3.0],"k":"a"}"#, &["v"]).unwrap(),
        r#"{"v":1,"k":"a"}"#
    );
    assert_eq!(
        mask(r#"{"k":"a","v":[1,2]}"#, &["v"]).unwrap(),
        r#"{"k":"a","v":1}"#
    );
    assert_eq!(
        mask(
            "{ \"k\" : \"a\" ,\n  \"v\" : [ 1 , 2 ] , \"z\": 5 }",
            &["v"]
        )
        .unwrap(),
        "{ \"k\" : \"a\" ,\n  \"v\" : 1 , \"z\": 5 }"
    );
}

#[test]
fn a_vector_in_a_nested_object_is_found_by_its_path() {
    assert_eq!(
        mask(
            r#"{"o":{"x":1,"v":[1,2]},"v":"keep","p":{"q":{"v":[3]}}}"#,
            &["o.v", "p.q.v"]
        )
        .unwrap(),
        r#"{"o":{"x":1,"v":1},"v":"keep","p":{"q":{"v":1}}}"#
    );
}

#[test]
fn strings_holding_structure_and_escapes_do_not_move_the_scan() {
    let source = r#"{"t":"a]},\"v\":[","u":{"w":"}"},"v":[1,2],"x":"\\"}"#;
    assert_eq!(
        mask(source, &["v"]).unwrap(),
        r#"{"t":"a]},\"v\":[","u":{"w":"}"},"v":1,"x":"\\"}"#
    );
}

#[test]
fn a_vector_given_as_a_number_or_a_string_is_replaced_too() {
    assert_eq!(
        mask(r#"{"v":5,"k":1}"#, &["v"]).unwrap(),
        r#"{"v":1,"k":1}"#
    );
    assert_eq!(
        mask(r#"{"v":"x y","k":1}"#, &["v"]).unwrap(),
        r#"{"v":1,"k":1}"#
    );
}

#[test]
fn no_field_to_mask_leaves_the_source_whole() {
    assert_eq!(mask(r#"{"k":"a"}"#, &[]).unwrap(), r#"{"k":"a"}"#);
}

#[test]
fn what_cannot_be_placed_is_refused_as_unsupported() {
    let why = |source: &str, fields: &[&str]| mask(source, fields).unwrap_err();
    assert_eq!(
        why(r#"{"o.v":[1]}"#, &["o.v"]),
        "derived source of a knn_vector: a dotted key"
    );
    assert_eq!(
        why(r#"{"o.p":{"v":[1]}}"#, &["o.p.v"]),
        "derived source of a knn_vector: a dotted key"
    );
    // A dotted key that no vector is under is no concern.
    assert_eq!(
        mask(r#"{"a.b":1,"v":[1]}"#, &["v"]).unwrap(),
        r#"{"a.b":1,"v":1}"#
    );
    assert_eq!(
        why(r#"{"a":[{"v":[1]}]}"#, &["a.v"]),
        "derived source of a knn_vector: a vector field is not at its path in the source"
    );
    assert_eq!(
        why(r#"{"\u0076":[1]}"#, &["v"]),
        "derived source of a knn_vector: an escaped key"
    );
    let bad = "derived source of a knn_vector: the source is not the JSON object it was parsed as";
    assert_eq!(why("[1]", &["v"]), bad);
    assert_eq!(why(r#"{"v":[1]"#, &["v"]), bad);
    assert_eq!(why(r#"{"k":"a" "v":[1]}"#, &["v"]), bad);
    assert_eq!(why(r#"{"k":"abc}"#, &["v"]), bad);
    // A value that ends with the source: unclosed array, unclosed string, none.
    assert_eq!(why(r#"{"v":[1,2"#, &["v"]), bad);
    assert_eq!(why(r#"{"v":"abc"#, &["v"]), bad);
    assert_eq!(why(r#"{"v":,"k":1}"#, &["v"]), bad);
    assert_eq!(why(r#"{"v":}"#, &["v"]), bad);
    assert_eq!(
        why(r#"{"v":[1]} x"#, &["v"]),
        "derived source of a knn_vector: bytes follow the source's object"
    );
}

#[test]
fn an_empty_object_and_empty_nested_object_are_walked() {
    assert_eq!(mask("{}", &[]).unwrap(), "{}");
    assert_eq!(
        mask(r#"{"o":{},"v":[1]}"#, &["v"]).unwrap(),
        r#"{"o":{},"v":1}"#
    );
}

#[test]
fn white_space_around_a_scalar_value_is_kept() {
    assert_eq!(
        mask("{\"v\" : 5 \n, \"k\":1}", &["v"]).unwrap(),
        "{\"v\" : 1 \n, \"k\":1}"
    );
    assert_eq!(mask("{\"v\":5 }", &["v"]).unwrap(), "{\"v\":1 }");
    assert_eq!(
        mask("{\"v\":true\t,\"k\":1}", &["v"]).unwrap(),
        "{\"v\":1\t,\"k\":1}"
    );
}

#[test]
fn a_dotted_key_that_only_shares_text_with_a_vector_path_is_no_concern() {
    assert_eq!(
        mask(r#"{"a.b":1,"a":{"bc":[1]}}"#, &["a.bc"]).unwrap(),
        r#"{"a.b":1,"a":{"bc":1}}"#
    );
}
