//! Unit layouts: the format byte for byte, lookups, splits, merges and the
//! layouts the Java refuses (AC-3, AC-5).

use oss_cluster::routing::{RoutingError, ShardMode, Split, UnitLayout};
use oss_format::UnitLayoutRecord;

use crate::{hash, rows, unit};

fn json(layout: &UnitLayout) -> String {
    let mut buf = vec![0u8; 300 * 1024];
    let n = layout.to_record().write(&mut buf).unwrap();
    String::from_utf8(buf[..n].to_vec()).unwrap()
}

#[test]
fn every_recorded_layout_reads_writes_and_answers_as_the_javas() {
    let rows = rows("layouts.txt");
    let mut layout: Option<UnitLayout> = None;
    let mut counts = [0usize; 7];
    for row in rows {
        match row[0].as_str() {
            "layout" => {
                let parsed = UnitLayout::parse(row[1].as_bytes()).unwrap();
                assert_eq!(json(&parsed), row[1], "written back byte for byte");
                layout = Some(parsed);
                counts[0] += 1;
            }
            "unit_for" => {
                let l = layout.as_ref().unwrap();
                let u = l.unit_for(hash(&row[1]));
                assert_eq!(u.id.to_string(), row[2], "unit for {}", row[1]);
                let h = hash(&row[1]);
                assert!(u.covers(h));
                // No other unit covers it.
                assert_eq!(l.units().iter().filter(|o| o.covers(h)).count(), 1);
                counts[1] += 1;
            }
            "split" => {
                let l = layout.as_ref().unwrap();
                let id: u32 = row[1].parse().unwrap();
                match l.plan_split(id) {
                    Ok(plan) => {
                        assert_eq!((plan.left, plan.right), (unit(&row[2]), unit(&row[3])));
                        let applied = l.apply(plan).unwrap();
                        assert_eq!(applied.unit_count(), l.unit_count() + 1);
                        assert!(applied.unit(id).is_err());
                    }
                    Err(_) => assert_eq!(row[2], "refused", "split of {id}"),
                }
                counts[2] += 1;
            }
            "units_of" => {
                let l = layout.as_ref().unwrap();
                let s: u32 = row[1].parse().unwrap();
                let got: Vec<String> = l
                    .units_of(s)
                    .map(|u| format!("{}:{}", u.id, l.needs_filter(u, s)))
                    .collect();
                assert_eq!(got, row[2..].to_vec(), "units of {s}");
                counts[3] += 1;
            }
            "merge" => {
                let l = layout.as_ref().unwrap();
                let (a, b): (u32, u32) = (row[1].parse().unwrap(), row[2].parse().unwrap());
                match l.merge(a, b) {
                    Ok(merged) => {
                        assert_eq!(*merged.unit(l.next_unit_id()).unwrap(), unit(&row[3]));
                        assert_eq!(merged.next_unit_id().to_string(), row[4]);
                    }
                    Err(_) => assert_eq!(row[3], "refused", "merge of {a} and {b}"),
                }
                counts[4] += 1;
            }
            "without_pending" => {
                assert_eq!(json(&layout.as_ref().unwrap().without_pending()), row[1]);
                counts[5] += 1;
            }
            "apply_pending" => {
                let l = layout.as_ref().unwrap();
                assert_eq!(json(&l.apply(*l.pending().unwrap()).unwrap()), row[1]);
                counts[6] += 1;
            }
            other => panic!("unknown row {other}"),
        }
    }
    assert_eq!(counts[0], 36);
    assert!(counts.iter().all(|c| *c >= 8), "{counts:?}");
}

/// Layouts the Java accepts only by coercing a JSON type it never writes:
/// a number as text, `1.0` for `1`, a hash as a number.
const COERCED: [usize; 3] = [43, 44, 45];

#[test]
fn every_refused_layout_is_refused_and_every_other_reads_as_the_javas() {
    let rows = rows("refusals.txt");
    assert_eq!(rows.len(), 47);
    let mut refused = 0;
    for (at, row) in rows.iter().enumerate() {
        let got = UnitLayout::parse(row[0].as_bytes());
        if row[1] == "refused" || COERCED.contains(&at) {
            assert!(got.is_err(), "case {at} should be refused: {}", row[0]);
            refused += 1;
        } else {
            assert_eq!(json(&got.unwrap()), row[1], "case {at}");
        }
    }
    assert_eq!(refused, 35);
}

#[test]
fn initial_layouts_are_the_javas_and_need_a_shard() {
    let fixed = UnitLayout::initial(ShardMode::Fixed, 3).unwrap();
    assert_eq!(fixed.unit_count(), 3);
    assert_eq!(fixed.next_unit_id(), 3);
    assert_eq!(fixed.declared_shards(), 3);
    assert_eq!(fixed.mode(), ShardMode::Fixed);
    let adaptive = UnitLayout::initial(ShardMode::Adaptive, 3).unwrap();
    assert_eq!((adaptive.unit_count(), adaptive.next_unit_id()), (1, 1));
    assert!(UnitLayout::initial(ShardMode::Fixed, 0).is_err());
    assert!(UnitLayout::initial(ShardMode::Adaptive, 0).is_err());
}

#[test]
fn a_pending_split_reserves_ids_and_blocks_another() {
    let layout = UnitLayout::initial(ShardMode::Adaptive, 2).unwrap();
    let plan = layout.plan_split(0).unwrap();
    let pending = layout.with_pending(plan).unwrap();
    assert_eq!(pending.next_unit_id(), 3);
    assert_eq!(pending.unit_count(), 1, "halves serve nothing yet");
    assert_eq!(pending.plan_split(0), Err(RoutingError::SplitPending));
    assert_eq!(
        pending.merge(0, 0),
        Err(RoutingError::Invalid("the units are not neighbours"))
    );
    let cleared = pending.without_pending();
    assert_eq!(cleared.pending(), None);
    assert_eq!(cleared.next_unit_id(), 3, "reserved ids stay used");
    let applied = pending.apply(plan).unwrap();
    assert_eq!(applied.pending(), None);
    // Applying another split keeps the pending one.
    let other = UnitLayout::initial(ShardMode::Fixed, 2).unwrap();
    let p0 = other.plan_split(0).unwrap();
    let with = other.with_pending(p0).unwrap();
    let p1 = Split {
        retired: *other.unit(1).unwrap(),
        left: oss_cluster::routing::Unit {
            id: 4,
            first: other.unit(1).unwrap().first,
            last: other.unit(1).unwrap().first,
        },
        right: oss_cluster::routing::Unit {
            id: 5,
            first: other.unit(1).unwrap().first + 1,
            last: u64::MAX,
        },
    };
    let both = with.apply(p1).unwrap();
    assert_eq!(both.pending(), Some(&p0));
    assert_eq!(both.next_unit_id(), 6);
    assert_eq!(both.merge(0, 4), Err(RoutingError::SplitPending));
    assert!(with.apply(p0).unwrap().unit(0).is_err());
    assert!(both.apply(p1).is_err(), "the retired unit is gone");
    // As the Java's `withPending`, a new plan replaces the pending one.
    assert_eq!(with.with_pending(p1).unwrap().pending(), Some(&p1));
}

#[test]
fn the_slot_limit_is_the_descriptors() {
    let mut layout = UnitLayout::initial(ShardMode::Fixed, 4095).unwrap();
    assert!(layout.within_shard_limit());
    layout = layout.apply(layout.plan_split(0).unwrap()).unwrap();
    assert_eq!(layout.next_unit_id(), 4097);
    assert!(!layout.within_shard_limit());
    let max = UnitLayout::initial(ShardMode::Fixed, 4096).unwrap();
    assert!(max.within_shard_limit());
}

#[test]
fn a_single_hash_unit_cannot_be_split_and_negative_counts_are_refused() {
    let mut layout = UnitLayout::initial(ShardMode::Adaptive, 1).unwrap();
    while layout.units()[0].first != layout.units()[0].last {
        let id = layout.units()[0].id;
        layout = layout.apply(layout.plan_split(id).unwrap()).unwrap();
    }
    assert!(layout.plan_split(layout.units()[0].id).is_err());
    assert_eq!(layout.unit_count(), 65);
    let mut record = UnitLayout::initial(ShardMode::Fixed, 1)
        .unwrap()
        .to_record();
    record.declared_shards = -1;
    assert!(UnitLayout::from_record(&record).is_err());
    let mut record = UnitLayout::initial(ShardMode::Fixed, 1)
        .unwrap()
        .to_record();
    record.units[0].id = -1;
    assert!(UnitLayout::from_record(&record).is_err());
}

#[test]
fn a_record_too_long_or_with_too_many_units_is_refused() {
    // Padded to the bound with trailing blanks, a layout reads; one byte
    // more is too large.
    let mut padded = br#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":[{"id":0,"first":"0","last":"ffffffffffffffff"}]}"#.to_vec();
    padded.resize(oss_format::MAX_UNIT_LAYOUT_BYTES, b' ');
    assert!(UnitLayoutRecord::parse(&padded).is_ok());
    padded.push(b' ');
    assert!(matches!(
        UnitLayoutRecord::parse(&padded),
        Err(oss_format::FormatError::TooLarge { .. })
    ));
    let mut text =
        String::from(r#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":["#);
    for i in 0..4097 {
        if i > 0 {
            text.push(',');
        }
        text.push_str(r#"{"id":0,"first":"0","last":"0"}"#);
    }
    text.push_str("]}");
    assert!(matches!(
        UnitLayoutRecord::parse(text.as_bytes()),
        Err(oss_format::FormatError::TooLarge { .. })
    ));
    // A layout at the slot limit fits the format's bound.
    let max = UnitLayout::initial(ShardMode::Fixed, 4096).unwrap();
    let mut buf = vec![0u8; oss_format::MAX_UNIT_LAYOUT_BYTES];
    let n = max.to_record().write(&mut buf).unwrap();
    assert_eq!(UnitLayout::parse(&buf[..n]).unwrap(), max);
    assert!(max.to_record().write(&mut buf[..10]).is_err());
}

#[test]
fn errors_say_what_was_refused() {
    use std::error::Error;
    let format = UnitLayout::parse(b"[1]").unwrap_err();
    assert!(matches!(format, RoutingError::Format(_)));
    assert!(format.source().is_some());
    assert!(format.to_string().contains("unit layout"));
    let invalid = UnitLayout::initial(ShardMode::Fixed, 0).unwrap_err();
    assert_eq!(
        invalid.to_string(),
        "invalid unit layout or routing: declared shards must be positive"
    );
    assert!(invalid.source().is_none());
    assert_eq!(
        RoutingError::SplitPending.to_string(),
        "a split is already pending"
    );
    assert_eq!(
        RoutingError::RoutingMissing.to_string(),
        "a routing value is required for a partitioned index"
    );
}
