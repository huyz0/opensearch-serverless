//! docs/specs/snapshot-aliases AC-1, AC-2: a take confirms each index's
//! aliased_by hint against the alias's own register, one read for each
//! distinct alias.

use std::borrow::Cow;

use oss_blob::S3Op::{Get, GetRange, List};
use oss_format::{AliasCapture, IndexRecord, MAX_ALIASES};
use oss_meta::{AliasRecord, DeleteMode, DescriptorStore, SnapshotTaker};

use crate::alias_mutations::{at_now, index, mutations};
use crate::point_in_time::buf;
use crate::{BASE, Backend, all, counted};

fn taker(b: &Backend) -> SnapshotTaker {
    SnapshotTaker::new(std::sync::Arc::clone(&b.store), BASE)
}

fn gets(n: &[(oss_blob::S3Op, u64)]) -> u64 {
    n.iter()
        .filter(|(op, _)| matches!(op, Get | GetRange))
        .map(|(_, c)| c)
        .sum()
}

fn alias(name: &str, write: bool) -> AliasCapture {
    AliasCapture {
        name: name.into(),
        write,
    }
}

fn plain(d: &DescriptorStore, name: &str, members: &[&'static str], write: Option<&'static str>) {
    let now = at_now;
    let mut m = mutations(d, &now);
    let mut record = AliasRecord::new(name, members);
    record.write_member = write.map(Cow::Borrowed);
    // The name is borrowed from the caller for the call only.
    m.create_alias(&record).unwrap();
}

#[test]
fn an_alias_is_confirmed_once_however_many_indices_name_it() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        index(&d, "logs");
        index(&d, "logs2");
        plain(&d, "shared", &["logs", "logs2"], Some("logs"));
        plain(&d, "mine", &["logs"], None);
        let mut described = vec![
            t.describe(&d, "logs", &mut buf()).unwrap(),
            t.describe(&d, "logs2", &mut buf()).unwrap(),
        ];
        let (_, n) = counted(b.meter(), || {
            t.capture_aliases(&d, &mut described, &mut buf()).unwrap();
        });
        assert_eq!(gets(&n), 2, "{:?} {n:?}", b.kind);
        assert!(n.iter().all(|(op, _)| *op != List), "{:?}", b.kind);
        assert_eq!(
            described[0].aliases,
            [alias("mine", false), alias("shared", true)],
            "{:?}",
            b.kind
        );
        assert_eq!(
            described[1].aliases,
            [alias("shared", false)],
            "{:?}",
            b.kind
        );
        let entry = described[0].capture(Vec::new(), true);
        assert_eq!(entry.aliases.len(), 2);
        assert!(described[0].capture(Vec::new(), false).aliases.is_empty());
    }
}

#[test]
fn a_hint_the_register_does_not_confirm_is_not_recorded() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        let mut record = IndexRecord::new("logs", "u", 1);
        record.aliased_by = vec![Cow::Borrowed("ghost")];
        d.create(&record, 1, &mut [0u8; 4096]).unwrap();
        let mut described = vec![t.describe(&d, "logs", &mut buf()).unwrap()];
        t.capture_aliases(&d, &mut described, &mut buf()).unwrap();
        assert!(described[0].aliases.is_empty(), "{:?}", b.kind);
        assert!(!described[0].hint_truncated);
    }
}

#[test]
fn a_data_stream_is_not_an_alias_of_its_backing_index() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        index(&d, "backing");
        let now = at_now;
        let mut stream = AliasRecord::new("stream", &["backing"]);
        stream.data_stream = true;
        stream.generation = 2;
        mutations(&d, &now).create_alias(&stream).unwrap();
        let mut described = vec![t.describe(&d, "backing", &mut buf()).unwrap()];
        t.capture_aliases(&d, &mut described, &mut buf()).unwrap();
        assert!(described[0].aliases.is_empty(), "{:?}", b.kind);
    }
}

#[test]
fn an_index_in_more_than_the_bound_keeps_the_first_names_and_says_so() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Conditional);
        let t = taker(&b);
        let names: Vec<String> = (0..=MAX_ALIASES).map(|i| format!("a{i:03}")).collect();
        let mut record = IndexRecord::new("logs", "u", 1);
        record.aliased_by = names.iter().map(|n| Cow::Borrowed(n.as_str())).collect();
        d.create(&record, 1, &mut [0u8; 4096]).unwrap();
        let mut described = vec![t.describe(&d, "logs", &mut buf()).unwrap()];
        assert!(described[0].hint_truncated, "{:?}", b.kind);
        assert_eq!(described[0].hint.len(), MAX_ALIASES);
        let (_, n) = counted(b.meter(), || {
            t.capture_aliases(&d, &mut described, &mut buf()).unwrap();
        });
        assert_eq!(gets(&n), 64, "{:?} {n:?}", b.kind);
        assert!(described[0].capture(Vec::new(), true).aliases_truncated);
    }
}
