//! docs/specs/alias-mutations/SPEC.md: changing aliases so every member's
//! hint names its alias before the alias is published, and a crashed
//! writer's half-done change is recovered.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::time::Duration;

use oss_blob::S3Op::{Get, GetRange, Put};
use oss_format::{AliasMutation, IndexRecord};
use oss_meta::{
    AliasAt, AliasClock, AliasMutations, AliasRecord, DeleteMode, DescriptorStore, MetaError,
    Target,
};

use crate::{all, counted};

pub(crate) const NOW: i64 = 1_763_164_800_000;
pub(crate) const TTL: i64 = 30_000;

thread_local! {
    static PAUSES: Cell<u32> = const { Cell::new(0) };
}

/// No wait, but a bounded number of them: a loop that never stops waiting
/// fails its test rather than hanging it.
pub(crate) fn no_pause(_: Duration) {
    PAUSES.with(|n| {
        assert!(n.get() < 100, "the wait is bounded");
        n.set(n.get() + 1);
    });
}

pub(crate) fn at_now() -> i64 {
    NOW
}

pub(crate) fn mutations<'a>(
    d: &'a DescriptorStore,
    now: &'a dyn Fn() -> i64,
) -> AliasMutations<'a> {
    AliasMutations::new(
        d,
        AliasClock {
            now_millis: now,
            pause: &no_pause,
        },
        TTL,
        4096,
    )
}

pub(crate) fn index(d: &DescriptorStore, name: &str) {
    d.create(&IndexRecord::new(name, "u", 1), NOW, &mut [0u8; 4096])
        .unwrap();
}

/// The aliases `name`'s descriptor hints at.
pub(crate) fn hints(d: &DescriptorStore, name: &str) -> Vec<String> {
    let mut buf = [0u8; 4096];
    let record = d.get(name, &mut buf).unwrap().expect("an index");
    record.aliased_by.iter().map(|a| a.to_string()).collect()
}

/// The alias `name` holds, with its generation, a pending creation
/// included.
pub(crate) fn raw(d: &DescriptorStore, name: &str) -> Option<AliasAt> {
    let mut buf = [0u8; 4096];
    let resolved = d.resolve_raw(name, &mut buf).unwrap();
    match resolved.target {
        Target::Alias(alias) => Some(AliasAt {
            alias: alias.into_owned(),
            generation: resolved.generation,
        }),
        Target::Index(_) | Target::Absent => None,
    }
}

pub(crate) fn names(members: &[&'static str]) -> Vec<Cow<'static, str>> {
    members.iter().map(|m| Cow::Borrowed(*m)).collect()
}

/// A writer that reserved `alias` to add `additions` and crashed, after
/// adding the hint to each of them: what it leaves in the store.
pub(crate) fn crashed(
    d: &DescriptorStore,
    alias: &AliasRecord<'static>,
    generation: u64,
    additions: &[&'static str],
    creating: bool,
    until: i64,
) -> u64 {
    let pending = AliasRecord {
        mutation: Some(AliasMutation {
            until,
            creating,
            additions: names(additions),
        }),
        ..alias.clone()
    };
    let mut buf = [0u8; 4096];
    let reserved = d
        .update_alias(&pending, generation, &mut buf)
        .unwrap()
        .unwrap();
    for added in additions {
        let mut read = [0u8; 4096];
        let current = d.resolve_raw(added, &mut read).unwrap();
        let Target::Index(mut record) = current.target else {
            panic!("{added} is not an index");
        };
        record.aliased_by.push(Cow::Borrowed(&alias.name));
        d.update(&record, current.generation, &mut buf)
            .unwrap()
            .unwrap();
    }
    reserved
}

#[test]
fn creating_an_alias_hints_every_member_before_it_is_published() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        index(&d, "logs-1");
        index(&d, "logs-2");
        let mut m = mutations(&d, &at_now);
        let alias = AliasRecord::new("logs", &["logs-1", "logs-2"]);
        let (g, sent) = counted(b.meter(), || m.create_alias(&alias).unwrap());
        // A read, the reservation's create, a read of the member and of the
        // reservation and a swap each member, and the publishing swap.
        assert_eq!(
            sent,
            b.budget(&[(Get, 5), (Put, 4)], &[(Get, 5), (GetRange, 3), (Put, 4)]),
            "{:?}",
            b.kind
        );
        assert_eq!(
            raw(&d, "logs"),
            Some(AliasAt {
                alias,
                generation: g
            })
        );
        assert_eq!(hints(&d, "logs-1"), ["logs"]);
        assert_eq!(hints(&d, "logs-2"), ["logs"]);
        assert!(matches!(
            m.create_alias(&AliasRecord::new("logs", &["logs-1"])),
            Err(MetaError::AlreadyExists)
        ));
    }
}

#[test]
fn a_member_that_is_no_index_undoes_the_creation() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        index(&d, "logs-1");
        let mut m = mutations(&d, &at_now);
        let got = m.create_alias(&AliasRecord::new("logs", &["logs-1", "nope"]));
        assert!(
            matches!(&got, Err(MetaError::MissingMember(n)) if n == "nope"),
            "{got:?}"
        );
        assert_eq!(raw(&d, "logs"), None, "{:?}", b.kind);
        assert!(hints(&d, "logs-1").is_empty());
        // The name is free again.
        m.create_alias(&AliasRecord::new("logs", &["logs-1"]))
            .unwrap();
    }
}

#[test]
fn an_update_hints_added_members_and_unhints_removed_ones() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        for name in ["a", "b", "c"] {
            index(&d, name);
        }
        let mut m = mutations(&d, &at_now);
        let before = AliasRecord::new("al", &["a", "b"]);
        let g = m.create_alias(&before).unwrap();
        let after = AliasRecord::new("al", &["b", "c"]);
        let (moved, sent) = counted(b.meter(), || m.update_alias(&before, &after, g).unwrap());
        let moved = moved.unwrap();
        // The reservation's swap; `c` read, the reservation read, and `c`
        // swapped; the publishing swap; `a` read, the alias read, and `a`
        // swapped. `b`, kept, costs nothing.
        assert_eq!(
            sent,
            b.budget(&[(Get, 4), (Put, 4)], &[(Get, 4), (GetRange, 4), (Put, 4)]),
            "{:?}",
            b.kind
        );
        assert_eq!(
            raw(&d, "al"),
            Some(AliasAt {
                alias: after.clone(),
                generation: moved
            })
        );
        assert!(hints(&d, "a").is_empty(), "{:?}", b.kind);
        assert_eq!(hints(&d, "b"), ["al"]);
        assert_eq!(hints(&d, "c"), ["al"]);
        // A stale generation loses at the reservation, and changes nothing.
        let (lost, sent) = counted(b.meter(), || m.update_alias(&before, &after, g).unwrap());
        assert_eq!(lost, None);
        // The store's refusal of a stale swap: a ranged read, no write.
        assert_eq!(sent, [(GetRange, 1)], "{:?}", b.kind);
        assert_eq!(hints(&d, "b"), ["al"]);
    }
}

#[test]
fn an_update_cannot_rename_an_alias() {
    let b = &all()[0];
    let d = b.descriptors(DeleteMode::Tombstone);
    let mut m = mutations(&d, &at_now);
    let before = AliasRecord::new("al", &["a"]);
    let (got, sent) = counted(b.meter(), || {
        m.update_alias(&before, &AliasRecord::new("other", &["a"]), 1)
    });
    assert!(matches!(got, Err(MetaError::InvalidArgument(_))), "{got:?}");
    assert!(sent.is_empty());
}

#[test]
fn a_data_stream_keeps_its_fields_through_a_mutation() {
    let b = &all()[0];
    let d = b.descriptors(DeleteMode::Tombstone);
    index(&d, "ds-1");
    index(&d, "ds-2");
    let mut m = mutations(&d, &at_now);
    let stream = AliasRecord {
        data_stream: true,
        generation: 2,
        timestamp_field: Some(Cow::Borrowed("@timestamp")),
        ..AliasRecord::new("ds", &["ds-1"])
    };
    let g = m.create_alias(&stream).unwrap();
    let rolled = stream.with_members(names(&["ds-1", "ds-2"]));
    m.update_alias(&stream, &rolled, g).unwrap().unwrap();
    let now = raw(&d, "ds").unwrap().alias;
    assert!(now.data_stream);
    assert_eq!((now.generation, now.write_index()), (2, Some("ds-2")));
    assert_eq!(hints(&d, "ds-2"), ["ds"]);
}

#[test]
fn an_expired_reservation_is_recovered_by_the_next_writer() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        for name in ["a", "b"] {
            index(&d, name);
        }
        let mut m = mutations(&d, &at_now);
        let alias = AliasRecord::new("al", &["a"]);
        let g = m.create_alias(&alias).unwrap();
        // A writer adding `b` crashed after preparing its hint.
        // A deadline of now has passed: the Java recovers once it is not after now.
        crashed(&d, &alias, g, &["b"], false, NOW);
        assert_eq!(hints(&d, "b"), ["al"]);
        let (read, sent) = counted(b.meter(), || m.resolve_for_mutation("al").unwrap());
        // The read; the published record restored; `b` read, the alias read,
        // `b` swapped; the read of what is left.
        assert_eq!(
            sent,
            b.budget(&[(Get, 4), (Put, 2)], &[(Get, 4), (GetRange, 2), (Put, 2)]),
            "{:?}",
            b.kind
        );
        let read = read.unwrap();
        assert_eq!(read.alias, alias, "{:?}", b.kind);
        assert!(hints(&d, "b").is_empty());
        assert_eq!(hints(&d, "a"), ["al"]);
        // The recovered record changes as any published one.
        m.update_alias(
            &read.alias,
            &AliasRecord::new("al", &["a", "b"]),
            read.generation,
        )
        .unwrap()
        .unwrap();
        assert_eq!(hints(&d, "b"), ["al"]);
    }
}

#[test]
fn an_abandoned_creation_is_deleted_by_the_next_writer() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        index(&d, "a");
        let mut m = mutations(&d, &at_now);
        let alias = AliasRecord::new("al", &["a"]);
        let pending = AliasRecord {
            mutation: Some(AliasMutation {
                until: NOW - 1,
                creating: true,
                additions: names(&["a"]),
            }),
            ..alias.clone()
        };
        d.create_alias(&pending, NOW, &mut [0u8; 4096]).unwrap();
        let (got, sent) = counted(b.meter(), || m.resolve_for_mutation("al").unwrap());
        // The read; the creation deleted, marker then tombstone; `a` read,
        // the alias read, `a` swapped as a fence; the read of what is left.
        assert_eq!(
            sent,
            b.budget(&[(Get, 4), (Put, 3)], &[(Get, 4), (GetRange, 2), (Put, 3)]),
            "{:?}",
            b.kind
        );
        assert_eq!(got, None);
        assert_eq!(raw(&d, "al"), None, "{:?}", b.kind);
        m.create_alias(&alias).unwrap();
        assert_eq!(hints(&d, "a"), ["al"]);
    }
}

#[test]
fn a_live_reservation_is_waited_for_then_left_alone() {
    let b = &all()[0];
    let d = b.descriptors(DeleteMode::Tombstone);
    index(&d, "a");
    index(&d, "b");
    let paused = RefCell::new(Vec::new());
    let pause = |p: Duration| {
        let mut paused = paused.borrow_mut();
        assert!(paused.len() < 20, "the wait is bounded");
        paused.push(p.as_millis());
    };
    let mut m = AliasMutations::new(
        &d,
        AliasClock {
            now_millis: &at_now,
            pause: &pause,
        },
        TTL,
        4096,
    );
    let alias = AliasRecord::new("al", &["a"]);
    let g = m.create_alias(&alias).unwrap();
    crashed(&d, &alias, g, &["b"], false, NOW + TTL);
    let read = m.resolve_for_mutation("al").unwrap().unwrap();
    // Pauses doubling to 64 ms, 255 ms in all, as the Java's 250 ms wait.
    assert_eq!(*paused.borrow(), [1, 2, 4, 8, 16, 32, 64, 64, 64]);
    assert!(read.alias.mutation_pending());
    assert_eq!(hints(&d, "b"), ["al"], "a live reservation keeps its hints");
    // A change from a pending record recovers instead, and answers none.
    assert_eq!(
        m.update_alias(
            &read.alias,
            &AliasRecord::new("al", &["b"]),
            read.generation
        )
        .unwrap(),
        None
    );
    assert!(!m.delete_alias(&read.alias, read.generation).unwrap());
    assert!(raw(&d, "al").unwrap().alias.mutation_pending());
}

#[test]
fn a_creation_still_pending_after_the_wait_reads_as_no_alias() {
    let b = &all()[0];
    let d = b.descriptors(DeleteMode::Tombstone);
    let waits = Cell::new(0);
    let pause = |_: Duration| {
        assert!(waits.get() < 20, "the wait is bounded");
        waits.set(waits.get() + 1);
    };
    let mut m = AliasMutations::new(
        &d,
        AliasClock {
            now_millis: &at_now,
            pause: &pause,
        },
        TTL,
        4096,
    );
    let pending = AliasRecord {
        mutation: Some(AliasMutation {
            until: NOW + TTL,
            creating: true,
            additions: names(&["a"]),
        }),
        ..AliasRecord::new("al", &["a"])
    };
    d.create_alias(&pending, NOW, &mut [0u8; 4096]).unwrap();
    assert_eq!(m.resolve_for_mutation("al").unwrap(), None);
    assert_eq!(waits.get(), 9);
    assert_eq!(m.resolve_for_mutation("nothing").unwrap(), None);
    // A published alias is read at once.
    index(&d, "a");
    d.create_alias(&AliasRecord::new("pub", &["a"]), NOW, &mut [0u8; 4096])
        .unwrap();
    assert!(m.resolve_for_mutation("pub").unwrap().is_some());
    assert_eq!(waits.get(), 9);
}

#[test]
fn deleting_an_alias_unhints_its_members() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        index(&d, "a");
        let mut m = mutations(&d, &at_now);
        let alias = AliasRecord::new("al", &["a"]);
        let g = m.create_alias(&alias).unwrap();
        let (got, sent) = counted(b.meter(), || m.delete_alias(&alias, g + 1).unwrap());
        // The marker, then the refused swap.
        assert_eq!(sent, [(GetRange, 1), (Put, 1)], "{:?}", b.kind);
        assert!(!got);
        assert_eq!(hints(&d, "a"), ["al"]);
        let (got, sent) = counted(b.meter(), || m.delete_alias(&alias, g).unwrap());
        // The marker and the tombstone; `a` read, the alias read, `a` swapped.
        assert_eq!(
            sent,
            b.budget(&[(Get, 2), (Put, 3)], &[(Get, 2), (GetRange, 2), (Put, 3)]),
            "{:?}",
            b.kind
        );
        assert!(got);
        assert_eq!(raw(&d, "al"), None, "{:?}", b.kind);
        assert!(hints(&d, "a").is_empty());
    }
}

#[test]
fn a_deleted_index_leaves_its_aliases_but_the_last_keeps_naming_it() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        for name in ["a", "b", "c"] {
            index(&d, name);
        }
        let mut m = mutations(&d, &at_now);
        m.create_alias(&AliasRecord::new("both", &["a", "b"]))
            .unwrap();
        m.create_alias(&AliasRecord::new("only", &["a"])).unwrap();
        let g = m.create_alias(&AliasRecord::new("busy", &["a"])).unwrap();
        // A live mutation adding `c` to `busy` is aborted, not waited for.
        crashed(
            &d,
            &AliasRecord::new("busy", &["a"]),
            g,
            &["c"],
            false,
            NOW + TTL,
        );
        m.create_alias(&AliasRecord::new("moved", &["b"])).unwrap();
        // A hint naming an alias that no longer names `a` costs its read.
        let (left, sent) = counted(b.meter(), || {
            m.detach_from_aliases("a", &names(&["moved"])).unwrap()
        });
        assert_eq!((left, sent), (0, vec![(Get, 1)]), "{:?}", b.kind);
        let hinted = names(&["both", "only", "busy", "gone"]);
        let (left, sent) = counted(b.meter(), || m.detach_from_aliases("a", &hinted).unwrap());
        // `both` read and swapped; `only` read; `busy` read, its mutation
        // aborted (a swap, then `c` read, `busy` read, `c` swapped) and read
        // again; `gone` read.
        assert_eq!(
            sent,
            b.budget(&[(Get, 7), (Put, 3)], &[(Get, 7), (GetRange, 3), (Put, 3)]),
            "{:?}",
            b.kind
        );
        assert_eq!(left, 0);
        assert_eq!(
            raw(&d, "both").unwrap().alias.indices,
            ["b"],
            "{:?}",
            b.kind
        );
        assert_eq!(raw(&d, "only").unwrap().alias.indices, ["a"]);
        let busy = raw(&d, "busy").unwrap().alias;
        assert!(!busy.mutation_pending());
        assert_eq!(busy.indices, ["a"]);
        assert!(hints(&d, "c").is_empty());
    }
}

#[test]
fn only_aliases_that_name_the_index_are_confirmed() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        index(&d, "a");
        index(&d, "b");
        let mut m = mutations(&d, &at_now);
        m.create_alias(&AliasRecord::new("al", &["a"])).unwrap();
        m.create_alias(&AliasRecord::new("other", &["b"])).unwrap();
        let hinted = names(&["al", "other", "gone", "b"]);
        let (confirmed, sent) = counted(b.meter(), || m.aliases_of("a", &hinted).unwrap());
        assert_eq!(confirmed, ["al"]);
        assert_eq!(sent, [(Get, 4)], "one read a hinted name, {:?}", b.kind);
    }
}
