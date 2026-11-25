//! docs/specs/resident-pages: the resident page bound's setting.

use super::*;

// docs/specs/resident-pages AC-1
#[test]
fn the_resident_page_bound_is_off_by_default_and_refuses_what_it_cannot_hold() {
    let off = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(off.resident_page_bytes, 0);
    let set = NodeConfig::from_lookup(lookup(&with(&[("OSS_RESIDENT_PAGE_BYTES", "1048576")])))
        .expect("valid");
    assert_eq!(set.resident_page_bytes, 1 << 20);
    for bad in ["abc", "1", "1048575", "-5", "1125899906842625"] {
        let err = NodeConfig::from_lookup(lookup(&with(&[("OSS_RESIDENT_PAGE_BYTES", bad)])))
            .expect_err(bad);
        assert!(err.contains("OSS_RESIDENT_PAGE_BYTES"), "{bad}: {err}");
    }
}
