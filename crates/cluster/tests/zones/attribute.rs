//! The zone is the awareness attribute, read as the Java's `Zones` (AC-1).

use std::collections::BTreeMap;

use oss_cluster::zones::{AWARENESS_ATTRIBUTES, DEFAULT_ATTRIBUTE, attribute, local_zone};

fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn the_zone_is_node_attr_zone_and_blank_is_none() {
    let set = settings(&[("node.attr.zone", "us-east-1a")]);
    assert_eq!(attribute(&set), DEFAULT_ATTRIBUTE);
    assert_eq!(local_zone(&set), Some("us-east-1a"));
    assert_eq!(local_zone(&settings(&[])), None, "no attribute, no zone");
    let blank = settings(&[("node.attr.zone", "  ")]);
    assert_eq!(local_zone(&blank), None, "blank is unknown");
    let padded = settings(&[("node.attr.zone", " az-b ")]);
    assert_eq!(local_zone(&padded), Some("az-b"), "trimmed");
}

#[test]
fn a_renamed_awareness_attribute_is_read_by_its_name() {
    let renamed = settings(&[
        (AWARENESS_ATTRIBUTES, "az, rack"),
        ("node.attr.az", "europe-west1-b"),
        ("node.attr.zone", "ignored"),
    ]);
    assert_eq!(attribute(&renamed), "az");
    assert_eq!(local_zone(&renamed), Some("europe-west1-b"));
    let skipped = settings(&[(AWARENESS_ATTRIBUTES, " , rack"), ("node.attr.rack", "r1")]);
    assert_eq!(
        attribute(&skipped),
        "rack",
        "a blank first entry is skipped"
    );
    assert_eq!(local_zone(&skipped), Some("r1"));
    let blanks = settings(&[(AWARENESS_ATTRIBUTES, " , "), ("node.attr.zone", "z")]);
    assert_eq!(
        attribute(&blanks),
        DEFAULT_ATTRIBUTE,
        "only blanks: the default"
    );
    assert_eq!(local_zone(&blanks), Some("z"));
    let named_unset = settings(&[(AWARENESS_ATTRIBUTES, "az"), ("node.attr.zone", "z")]);
    assert_eq!(
        local_zone(&named_unset),
        None,
        "the named attribute is unset"
    );
}

#[test]
fn the_array_form_is_read_when_the_key_is_not_set() {
    let array = settings(&[
        ("cluster.routing.allocation.awareness.attributes.0", " "),
        ("cluster.routing.allocation.awareness.attributes.1", " az "),
        ("cluster.routing.allocation.awareness.attributes.2", "rack"),
        ("node.attr.az", "us-west-2c"),
    ]);
    assert_eq!(attribute(&array), "az");
    assert_eq!(local_zone(&array), Some("us-west-2c"));
    let gap = settings(&[
        ("cluster.routing.allocation.awareness.attributes.1", "az"),
        ("node.attr.zone", "z"),
    ]);
    assert_eq!(attribute(&gap), DEFAULT_ATTRIBUTE, "the list starts at .0");
    let only_blanks = settings(&[("cluster.routing.allocation.awareness.attributes.0", "")]);
    assert_eq!(attribute(&only_blanks), DEFAULT_ATTRIBUTE);
}
