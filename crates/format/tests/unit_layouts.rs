//! Unit layouts read and written as the Java's `UnitLayoutFormat` does
//! (docs/specs/routing/SPEC.md, AC-3 and AC-5), over `fixtures/routing/`.

// ARITH: indexes and counts here are fixture sizes; an overflow panics the
// test in a debug build, which fails it.
#![allow(clippy::disallowed_methods, clippy::arithmetic_side_effects)]

use std::path::PathBuf;

use oss_format::{
    FormatError, MAX_SHARDS, MAX_UNIT_LAYOUT_BYTES, ShardMode, UnitLayoutRecord, UnitRecord,
};

fn rows(name: &str) -> Vec<Vec<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/routing")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
        .lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

fn written(record: &UnitLayoutRecord) -> String {
    let mut buf = vec![0u8; MAX_UNIT_LAYOUT_BYTES];
    let n = record.write(&mut buf).unwrap();
    String::from_utf8(buf[..n].to_vec()).unwrap()
}

#[test]
fn every_recorded_layout_reads_and_writes_back_byte_for_byte() {
    let mut layouts = 0;
    for file in ["layouts.txt", "units.txt"] {
        for row in rows(file) {
            if row.len() == 2 && row[1].starts_with('{') {
                let record = UnitLayoutRecord::parse(row[1].as_bytes()).unwrap();
                assert_eq!(written(&record), row[1]);
                layouts += 1;
            }
        }
    }
    assert!(layouts >= 39, "{layouts}");
}

#[test]
fn a_recorded_layout_reads_to_its_fields() {
    let rows = rows("units.txt");
    let record = UnitLayoutRecord::parse(rows[0][1].as_bytes()).unwrap();
    assert_eq!(record.mode, ShardMode::Fixed);
    assert_eq!((record.declared_shards, record.next_unit_id), (5, 5));
    assert_eq!(record.units.len(), 5);
    assert_eq!(
        record.units[1],
        UnitRecord {
            id: 1,
            first: 0x3333_3333_3333_3334,
            last: 0x6666_6666_6666_6666
        }
    );
    assert_eq!(record.pending, None);
    let pending = self::rows("layouts.txt")
        .into_iter()
        .find(|r| r[0] == "layout" && r[1].contains("pending_split"))
        .unwrap();
    let record = UnitLayoutRecord::parse(pending[1].as_bytes()).unwrap();
    let [retired, left, right] = record.pending.unwrap();
    assert_eq!((left.first, right.last), (retired.first, retired.last));
    assert_eq!(left.last + 1, right.first);
}

/// Refused by the record itself; the rest of the Java's refusals break
/// layout rules that `oss-cluster` checks. 43 to 45 are the three the Java
/// takes only by coercing a JSON type it never writes.
const REFUSED_HERE: [usize; 20] = [
    3, 4, 5, 6, 7, 8, 9, 23, 24, 25, 26, 27, 31, 40, 41, 42, 43, 44, 45, 46,
];

#[test]
fn the_records_refusals_are_the_javas_less_the_layout_rules() {
    let rows = rows("refusals.txt");
    assert_eq!(rows.len(), 47);
    for (at, row) in rows.iter().enumerate() {
        let got = UnitLayoutRecord::parse(row[0].as_bytes());
        if REFUSED_HERE.contains(&at) {
            assert!(got.is_err(), "case {at} should be refused: {}", row[0]);
        } else {
            let record = got.unwrap_or_else(|e| panic!("case {at}: {e}"));
            // The Java sorts units by hash; the record keeps them as read.
            if row[1] != "refused" && at != 17 {
                assert_eq!(written(&record), row[1], "case {at}");
            }
        }
    }
}

#[test]
fn every_field_is_required_and_typed() {
    let good = br#"{"mode":"adaptive","declared_shards":2,"next_unit_id":3,"units":[{"id":2,"first":"0","last":"ffffffffffffffff"}]}"#;
    let record = UnitLayoutRecord::parse(good).unwrap();
    assert_eq!(record.mode, ShardMode::Adaptive);
    assert_eq!((record.declared_shards, record.next_unit_id), (2, 3));
    assert_eq!(record.units[0].id, 2);
    for bad in [
        r#"{"mode":"fixed","declared_shards":1,"next_unit_id":0,"units":[{"id":0,"first":"0","last":"0"}]}"#,
        r#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":[{"id":0,"first":"0","last":"0"}],"pending_split":[{"id":0,"first":"0","last":"0"}]}"#,
        r#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":[{"id":0,"first":"0","last":"0","id":1}]}"#,
        r#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":[{"id":0,"first":"0","last":"0"}],"units":[]}"#,
        r#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":[3]}"#,
        r#"{"mode":"fixed","declared_shards":1,"next_unit_id":4294967296,"units":[{"id":0,"first":"0","last":"0"}]}"#,
        r#"{"mode":1,"declared_shards":1,"next_unit_id":1,"units":[{"id":0,"first":"0","last":"0"}]}"#,
        r#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":[{"id":0,"first":"","last":"0"}]}"#,
    ] {
        assert!(UnitLayoutRecord::parse(bad.as_bytes()).is_err(), "{bad}");
    }
    // A three-unit pending split and an empty one are both read.
    let with = br#"{"mode":"adaptive","declared_shards":1,"next_unit_id":3,"units":[{"id":0,"first":"0","last":"ffffffffffffffff"}],"pending_split":[{"id":0,"first":"0","last":"ffffffffffffffff"},{"id":1,"first":"0","last":"7fffffffffffffff"},{"id":2,"first":"8000000000000000","last":"ffffffffffffffff"}]}"#;
    assert_eq!(
        UnitLayoutRecord::parse(with).unwrap().pending.unwrap()[2].id,
        2
    );
}

#[test]
fn a_layout_at_the_slot_limit_fits_and_past_it_is_refused() {
    let unit = |id: u32| UnitRecord {
        id: i32::try_from(id).unwrap(),
        first: u64::MAX - u64::from(MAX_SHARDS) + u64::from(id),
        last: u64::MAX - u64::from(MAX_SHARDS) + u64::from(id),
    };
    let mut record = UnitLayoutRecord {
        mode: ShardMode::Fixed,
        declared_shards: 4096,
        next_unit_id: i32::MAX,
        units: (0..MAX_SHARDS).map(unit).collect(),
        pending: Some([unit(1), unit(2), unit(3)]),
    };
    // The longest form every unit can take: 16-digit hashes, 4-digit ids.
    for u in &mut record.units {
        u.id = 1000 + u.id % 9000;
        u.first |= 1 << 63;
        u.last |= 1 << 63;
    }
    let mut buf = vec![0u8; MAX_UNIT_LAYOUT_BYTES];
    let n = record.write(&mut buf).unwrap();
    assert!(n > 256 * 1024, "{n}");
    assert_eq!(UnitLayoutRecord::parse(&buf[..n]).unwrap(), record);
    assert!(matches!(
        record.write(&mut buf[..n - 1]),
        Err(FormatError::TooLarge { actual, .. }) if actual == n as u64
    ));
    // One unit more than the descriptor's slots is refused as too large.
    let mut text =
        String::from(r#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":["#);
    for i in 0..=MAX_SHARDS {
        if i > 0 {
            text.push(',');
        }
        text.push_str(r#"{"id":0,"first":"0","last":"0"}"#);
    }
    text.push_str("]}");
    assert!(matches!(
        UnitLayoutRecord::parse(text.as_bytes()),
        Err(FormatError::TooLarge { .. })
    ));
    // Padded to the byte bound a layout reads; one byte more does not.
    let mut padded = br#"{"mode":"fixed","declared_shards":1,"next_unit_id":1,"units":[{"id":0,"first":"0","last":"ffffffffffffffff"}]}"#.to_vec();
    padded.resize(MAX_UNIT_LAYOUT_BYTES, b' ');
    assert!(UnitLayoutRecord::parse(&padded).is_ok());
    padded.push(b' ');
    assert!(matches!(
        UnitLayoutRecord::parse(&padded),
        Err(FormatError::TooLarge { .. })
    ));
}
