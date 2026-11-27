//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::check;
use crate::mapping::Mapping;

fn mapping() -> Mapping {
    Mapping::from_json(
        r#"{"properties":{"o":{"properties":{"a":{"type":"keyword"}}},"s":{"type":"keyword","store":true},"k":{"type":"keyword"}}}"#,
    )
    .expect("mapping")
}

fn run(names: &[&str]) -> Result<(), String> {
    let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    check(&names, &mapping()).map_err(|e| format!("{:?}", e.failure))
}

#[test]
fn plain_and_unmapped_names_pass() {
    assert!(run(&["k", "missing", "_source", "_none_", "o.a"]).is_ok());
}

#[test]
fn an_object_is_not_a_leaf() {
    let e = run(&["o"]).expect_err("object");
    assert!(e.contains("field [o] isn't a leaf field"), "{e}");
}

#[test]
fn a_stored_field_is_refused_by_name_or_pattern() {
    for n in ["s", "s*", "*"] {
        let e = run(&[n]).expect_err(n);
        assert!(e.contains("not supported by this node yet"), "{e}");
    }
}
