//! The vector warm's variable.

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
