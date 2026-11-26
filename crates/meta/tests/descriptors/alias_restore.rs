//! docs/specs/snapshot-aliases AC-6: a restore adds its indices to an alias
//! in one mutation, and a restored index writes only where nothing did.

use oss_blob::S3Op::Put;
use oss_meta::{AliasRecord, DeleteMode, MetaError};

use crate::alias_mutations::{at_now, hints, index, mutations, raw};
use crate::{all, counted};

fn m(name: &str, write: bool) -> (String, bool) {
    (name.to_owned(), write)
}

#[test]
fn a_new_alias_is_created_with_every_member_and_the_first_claimant_writes() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        index(&d, "a");
        index(&d, "b");
        let now = at_now;
        mutations(&d, &now)
            .add_restored("al", &[m("a", true), m("b", true)])
            .unwrap();
        let got = raw(&d, "al").unwrap().alias;
        assert_eq!(got.indices, ["a", "b"], "{:?}", b.kind);
        assert_eq!(got.write_index(), Some("a"));
        assert_eq!(hints(&d, "a"), ["al"]);
        assert_eq!(hints(&d, "b"), ["al"]);
    }
}

#[test]
fn an_existing_alias_with_a_write_target_keeps_it() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        index(&d, "old");
        index(&d, "a");
        index(&d, "c");
        let now = at_now;
        // One member, no flag: it still writes through that member.
        mutations(&d, &now)
            .create_alias(&AliasRecord::new("implicit", &["old"]))
            .unwrap();
        mutations(&d, &now)
            .add_restored("implicit", &[m("a", true)])
            .unwrap();
        let got = raw(&d, "implicit").unwrap().alias;
        assert_eq!(got.indices, ["old", "a"], "{:?}", b.kind);
        assert_eq!(got.writes_to(), Some("old"));
        // An explicit write member.
        let mut explicit = AliasRecord::new("explicit", &["old"]);
        explicit.write_member = Some("old".into());
        mutations(&d, &now).create_alias(&explicit).unwrap();
        mutations(&d, &now)
            .add_restored("explicit", &[m("c", true)])
            .unwrap();
        assert_eq!(
            raw(&d, "explicit").unwrap().alias.write_index(),
            Some("old")
        );
    }
}

#[test]
fn a_multi_member_alias_with_no_write_target_gives_it_to_the_first_claimant() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        for i in ["x", "y", "a", "c"] {
            index(&d, i);
        }
        let now = at_now;
        mutations(&d, &now)
            .create_alias(&AliasRecord::new("al", &["x", "y"]))
            .unwrap();
        mutations(&d, &now)
            .add_restored("al", &[m("a", false), m("c", true)])
            .unwrap();
        let got = raw(&d, "al").unwrap().alias;
        assert_eq!(got.indices, ["x", "y", "a", "c"], "{:?}", b.kind);
        assert_eq!(got.write_index(), Some("c"));
    }
}

#[test]
fn the_cost_is_one_mutation_for_all_members_and_a_repeat_changes_nothing() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        for i in ["a", "b", "c"] {
            index(&d, i);
        }
        let now = at_now;
        mutations(&d, &now)
            .add_restored("al", &[m("a", false)])
            .unwrap();
        let (_, n) = counted(b.meter(), || {
            mutations(&d, &now)
                .add_restored("al", &[m("b", false), m("c", false)])
                .unwrap();
        });
        let puts: u64 = n.iter().filter(|(op, _)| *op == Put).map(|(_, c)| c).sum();
        // reserve, two hints, publish: independent of how many members.
        assert!(puts <= 4 + 2, "{:?} {n:?}", b.kind);
        let (_, n) = counted(b.meter(), || {
            mutations(&d, &now)
                .add_restored("al", &[m("b", false)])
                .unwrap();
        });
        assert!(n.iter().all(|(op, _)| *op != Put), "{:?} {n:?}", b.kind);
    }
}

#[test]
fn a_data_stream_an_index_name_and_a_missing_member_are_refused() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        index(&d, "a");
        index(&d, "other");
        let now = at_now;
        let mut stream = AliasRecord::new("stream", &["a"]);
        stream.data_stream = true;
        stream.generation = 1;
        mutations(&d, &now).create_alias(&stream).unwrap();
        assert!(
            mutations(&d, &now)
                .add_restored("stream", &[m("other", false)])
                .is_err(),
            "{:?}",
            b.kind
        );
        assert!(
            matches!(
                mutations(&d, &now).add_restored("other", &[m("a", false)]),
                Err(MetaError::AlreadyExists)
            ),
            "{:?}",
            b.kind
        );
        assert!(
            matches!(
                mutations(&d, &now).add_restored("new", &[m("ghost", false)]),
                Err(MetaError::MissingMember(_))
            ),
            "{:?}",
            b.kind
        );
    }
}
