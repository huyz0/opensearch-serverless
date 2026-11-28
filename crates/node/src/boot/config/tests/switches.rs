//! The vector warm's and the nested objects' variables.

use super::{FS, lookup, with};
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
    let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_READER_RAMP_MAX_SECS", "0")])))
        .expect("valid");
    super::super::apply::apply(&off, &mut node);
    assert!(!oss_reader::vector_warm::at_open());
}

// docs/specs/nested-fields AC-9
#[test]
fn nested_is_on_by_default_and_turned_off_by_its_variable() {
    let c = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    assert!(c.nested);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_NESTED", "false")]))).expect("valid");
    assert!(!c.nested);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_NESTED", "yes")]))).is_err());
}

// docs/specs/geospatial AC-11
#[test]
fn geo_fields_are_on_by_default_and_turned_off_by_their_variable() {
    let c = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    assert!(c.geo);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_GEOSPATIAL_FIELDS", "false")])))
        .expect("valid");
    assert!(!c.geo);
    assert!(NodeConfig::from_lookup(lookup(&with(&[("OSS_GEOSPATIAL_FIELDS", "yes")]))).is_err());
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

// docs/specs/reader-slow-start, Switch
#[test]
fn the_reader_ramp_is_on_by_default_off_at_zero_and_its_bounds_are_checked() {
    let c = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    assert_eq!((c.reader_ramp.max, c.reader_ramp.min), (300, 30));
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_READER_RAMP_MAX_SECS", "0")])))
        .expect("valid");
    assert_eq!(c.reader_ramp.max, 0);
    for bad in [
        ("OSS_READER_RAMP_MAX_SECS", "-1"),
        ("OSS_READER_RAMP_MAX_SECS", "x"),
    ] {
        assert!(
            NodeConfig::from_lookup(lookup(&with(&[bad]))).is_err(),
            "{bad:?}"
        );
    }
    let min_above = [
        ("OSS_READER_RAMP_MAX_SECS", "20"),
        ("OSS_READER_RAMP_MIN_SECS", "40"),
    ];
    assert!(NodeConfig::from_lookup(lookup(&with(&min_above))).is_err());
}

// docs/specs/reader-slow-start, Switch
#[test]
fn the_ramp_variables_reach_placement() {
    let mut node = crate::assemble::Config::new("", "n1", [0; 6]);
    let on = NodeConfig::from_lookup(lookup(&with(&[]))).expect("valid");
    super::super::apply::apply(&on, &mut node);
    let set = oss_cluster::placement::ramp_config();
    assert_eq!((set.min_ms, set.max_ms), (30_000, 300_000));
    let off = NodeConfig::from_lookup(lookup(&with(&[("OSS_READER_RAMP_MAX_SECS", "0")])))
        .expect("valid");
    super::super::apply::apply(&off, &mut node);
    assert!(!oss_cluster::placement::ramp_config().is_on());
}

// docs/specs/ml-commons AC-17
#[test]
fn ml_commons_is_on_by_default_and_turned_off_by_its_variable() {
    let c = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert!(c.plugins.ml);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_ML", "false")]))).expect("valid");
    assert!(!c.plugins.ml);
    let c = NodeConfig::from_lookup(lookup(&with(&[("OSS_ML", "maybe")])));
    assert!(c.is_err());
}
