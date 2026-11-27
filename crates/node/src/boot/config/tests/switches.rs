//! The vector warm's and the nested objects' variables.

use super::{lookup, with};
use crate::boot::config::*;

// docs/specs/reader-vector-warm AC-5
#[test]
fn the_vector_warm_is_at_the_first_knn_search_unless_asked_to_be_at_open() {
    let c = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    assert!(!c.vector_warm_at_open);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_VECTOR_WARM_AT_OPEN", "true")])))
        .expect("valid");
    assert!(c.vector_warm_at_open);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_VECTOR_WARM_AT_OPEN", "yes")]))).is_err());
}

// docs/specs/reader-vector-warm AC-5
#[test]
fn the_variable_reaches_the_engine() {
    let mut node = crate::assemble::Config::new("", "n1", [0; 6]);
    let on = NodeConfig::from_lookup(lookup(&with(&[("OSS_VECTOR_WARM_AT_OPEN", "true")])))
        .expect("valid");
    super::super::apply::apply(&on, &mut node);
    assert!(oss_reader::vector_warm::at_open());
    let off = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    super::super::apply::apply(&off, &mut node);
    assert!(!oss_reader::vector_warm::at_open());
}

// docs/specs/nested-fields AC-9
#[test]
fn nested_is_off_by_default_and_turned_on_by_its_variable() {
    let c = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    assert!(!c.nested);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_NESTED", "true")]))).expect("valid");
    assert!(c.nested);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_NESTED", "yes")]))).is_err());
}

// docs/specs/painless-script-score AC-1
#[test]
fn painless_source_is_refused_unless_its_variable_turns_the_compiler_on() {
    let c = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    assert!(!c.painless_native_compile);
    let on = NodeConfig::from_lookup(lookup(&with(&[(
        "OSS_SCRIPTING_PAINLESS_NATIVE_COMPILE",
        "true",
    )])))
    .expect("valid");
    assert!(on.painless_native_compile);
    assert!(
        NodeConfig::from_lookup(lookup(&with(&[(
            "OSS_SCRIPTING_PAINLESS_NATIVE_COMPILE",
            "yes"
        )])))
        .is_err()
    );
}
