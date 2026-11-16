//! Name records written byte for byte as the Java shell writes them
//! (docs/specs/descriptor-store/SPEC.md, AC-1 to AC-4).

mod common;

use std::borrow::Cow;

use common::*;
use oss_format::{
    AliasRecord, EMPTY_SETTINGS, FormatError, IndexRecord, MAX_TOMBSTONE_BYTES, NameRecord,
    Register, Tombstone, descriptor_key, tombstone_bucket, tombstone_bucket_start,
    tombstone_marker_key,
};

fn written(record: &IndexRecord<'_>) -> Vec<u8> {
    let mut buf = vec![0u8; 4096];
    let n = record.write(&mut buf).unwrap();
    buf.truncate(n);
    buf
}

fn index(value: &[u8]) -> IndexRecord<'_> {
    match NameRecord::parse(value).unwrap() {
        NameRecord::Index(record) => record,
        other => panic!("an index, not {other:?}"),
    }
}

fn refused(value: &str) -> FormatError {
    NameRecord::parse(value.as_bytes()).expect_err(value)
}

const LAYOUT: &str = r#"{"mode":"adaptive","declared_shards":2,"next_unit_id":2,"units":[{"id":0,"first":"0","last":"7fffffffffffffff"},{"id":1,"first":"8000000000000000","last":"ffffffffffffffff"}]}"#;
const SETTINGS: &str =
    r#"{"index":{"analysis":{"filter":["icu_folding"]},"refresh_interval":"1s"}}"#;
const MAPPING: &str = r#"{"properties":{"path":{"type":"keyword","null_value":"a/b\\c"},"téxt":{"type":"text"}},"_meta":{"note":"tab\there\nnew"}}"#;

fn full() -> IndexRecord<'static> {
    IndexRecord {
        mapping: Some(Cow::Borrowed(MAPPING)),
        mapping_version: 3,
        settings_version: 7,
        created_at_millis: 1_763_164_800_123,
        created_version_id: 137_217_827,
        aliased_by: vec![Cow::Borrowed("logs"), Cow::Borrowed("logs-été")],
        unit_layout: Some(LAYOUT.as_bytes()),
        settings: SETTINGS.as_bytes(),
        ..IndexRecord::new("logs-2026.11", "Zq3vX0yJT8mN1bW2cD4eFg", 2)
    }
}

fn bucketed() -> IndexRecord<'static> {
    IndexRecord {
        mapping_version: -1,
        created_at_millis: i64::MAX,
        created_version_id: i32::MIN,
        bucket: Some(Cow::Borrowed("tenant-7.b3")),
        ..IndexRecord::new("tiny", "u-1", 1)
    }
}

#[test]
fn the_shells_descriptor_writes_back_byte_for_byte() {
    let bytes = blob(&descriptor_key(BASE, INDEX).unwrap());
    let value = Register::parse(&bytes).unwrap().value;
    let record = index(value);
    assert_eq!(record.name, INDEX);
    assert_eq!(record.uuid, UUID);
    assert_eq!(
        record.mapping.as_deref(),
        Some(r#"{"properties":{"msg":{"type":"text"},"n":{"type":"long"}}}"#),
        "the mapping decoded"
    );
    assert_eq!(record.settings, EMPTY_SETTINGS);
    assert_eq!(written(&record), value);
}

#[test]
fn every_field_writes_as_jackson_writes_it() {
    for (file, record) in [
        ("descriptor-layout.json", full()),
        ("descriptor-bucket.json", bucketed()),
    ] {
        let recorded = jackson(file);
        assert_eq!(
            String::from_utf8(written(&record)).unwrap(),
            String::from_utf8(recorded.clone()).unwrap(),
            "{file}"
        );
        assert_eq!(index(&recorded), record, "{file} parses back");
    }
}

#[test]
fn strings_escape_as_jackson_escapes_them() {
    let mut every: String = (0u8..0x80).map(char::from).collect();
    every.push_str("\u{80}\u{9f}\u{e9}\u{2028}\u{2029}\u{1F600}\u{FEFF}");
    let record = IndexRecord {
        mapping: Some(Cow::Borrowed(&every)),
        ..IndexRecord::new("x", "u", 1)
    };
    let bytes = written(&record);
    let mut expected = br#"{"name":"x","uuid":"u","number_of_shards":1,"mapping":"#.to_vec();
    expected.extend_from_slice(&jackson("every-character.json"));
    expected.extend_from_slice(br#","settings":{}}"#);
    assert_eq!(
        String::from_utf8(bytes.clone()).unwrap(),
        String::from_utf8(expected).unwrap()
    );
    assert_eq!(index(&bytes).mapping.as_deref(), Some(every.as_str()));
}

#[test]
fn a_record_the_java_refuses_is_refused() {
    let base = r#""name":"a","uuid":"u","number_of_shards":1"#;
    for (value, why) in [
        (r#"{"uuid":"u","number_of_shards":1}"#, "no name"),
        (r#"{"name":"a","number_of_shards":1}"#, "no uuid"),
        (r#"{"name":"a","uuid":"u"}"#, "no shards"),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":0}"#,
            "no shard",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":-1}"#,
            "negative",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":4097}"#,
            "too many",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":4294967297}"#,
            "past u32",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":2,"bucket":"b"}"#,
            "bucket with shards",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1,"unit_layout":{},"bucket":"b"}"#,
            "bucket with a layout",
        ),
        (
            r#"{"name":"a","name":"b","uuid":"u","number_of_shards":1}"#,
            "repeated",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1,"settings":{},"settings":{}}"#,
            "repeated object",
        ),
        (r#"{"name":1,"uuid":"u","number_of_shards":1}"#, "name type"),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":"1"}"#,
            "shards type",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1.0}"#,
            "shards fraction",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1,"created_version_id":2147483648}"#,
            "version id past int",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1,"settings":"x"}"#,
            "settings type",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1,"unit_layout":[]}"#,
            "layout type",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1,"aliased_by":"a"}"#,
            "aliases type",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1,"aliased_by":[1]}"#,
            "alias type",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1"#,
            "truncated",
        ),
        (
            r#"{"name":"a","uuid":"u","number_of_shards":1}x"#,
            "trailing",
        ),
        (r#"["name"]"#, "not an object"),
    ] {
        assert!(
            matches!(refused(value), FormatError::Corrupt { .. }),
            "{why}: {value}"
        );
    }
    // The base is accepted: only the defect above is refused.
    assert_eq!(index(format!("{{{base}}}").as_bytes()).number_of_shards, 1);
    assert_eq!(
        index(br#"{"name":"a","uuid":"u","number_of_shards":4096}"#).number_of_shards,
        4096
    );
}

#[test]
fn unknown_fields_are_skipped() {
    let record = index(
        br#"{"later":{"a":[1,{"b":null}]},"name":"a","n":1.5,"uuid":"u","list":[],"number_of_shards":3,"flag":true}"#,
    );
    assert_eq!(record, IndexRecord::new("a", "u", 3));
}

#[test]
fn an_alias_or_a_tombstone_is_told_apart() {
    let alias = br#"{"alias":"al","indices":["a"]}"#;
    assert_eq!(
        NameRecord::parse(alias).unwrap(),
        NameRecord::Alias(AliasRecord::new("al", &["a"]))
    );
    // The discriminator decides, and the record must then be an alias.
    assert!(matches!(
        NameRecord::parse(br#"{"name":"al","alias":{"indices":["a"]}}"#),
        Err(FormatError::Corrupt { .. })
    ));
    // A root field decides, not one inside an object.
    let nested = br#"{"name":"a","uuid":"u","number_of_shards":1,"settings":{"alias":1}}"#;
    assert!(matches!(
        NameRecord::parse(nested).unwrap(),
        NameRecord::Index(_)
    ));
    assert!(matches!(
        NameRecord::parse(br#"{"alias":1"#),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        NameRecord::parse(br#"{"alias":1}{}"#),
        Err(FormatError::Corrupt { .. })
    ));
    assert_eq!(
        NameRecord::parse(br#"{"tombstone":true,"deleted_at":5}"#).unwrap(),
        NameRecord::Tombstone(Tombstone {
            deleted_at: 5,
            reaping_at: 0
        })
    );
}

#[test]
fn a_short_buffer_says_how_long_one_must_be() {
    let record = full();
    let needed = written(&record).len();
    let mut short = vec![0u8; needed - 1];
    assert_eq!(
        record.write(&mut short),
        Err(FormatError::TooLarge {
            what: "index descriptor",
            limit: (needed - 1) as u64,
            actual: needed as u64,
        })
    );
    let mut exact = vec![0u8; needed];
    assert_eq!(record.write(&mut exact), Ok(needed));
}

#[test]
fn a_record_the_java_would_not_build_is_not_written() {
    let mut buf = [0u8; 256];
    for (record, why) in [
        (IndexRecord::new("a", "u", 0), "no shard"),
        (IndexRecord::new("a", "u", 4097), "too many"),
        (
            IndexRecord {
                bucket: Some(Cow::Borrowed("b")),
                ..IndexRecord::new("a", "u", 2)
            },
            "bucket with shards",
        ),
        (
            IndexRecord {
                bucket: Some(Cow::Borrowed("b")),
                unit_layout: Some(b"{}"),
                ..IndexRecord::new("a", "u", 1)
            },
            "bucket with a layout",
        ),
        (
            IndexRecord {
                settings: b"[]",
                ..IndexRecord::new("a", "u", 1)
            },
            "settings not an object",
        ),
        (
            IndexRecord {
                settings: b"{} {}",
                ..IndexRecord::new("a", "u", 1)
            },
            "two objects",
        ),
        (
            IndexRecord {
                unit_layout: Some(b"{"),
                ..IndexRecord::new("a", "u", 1)
            },
            "layout truncated",
        ),
    ] {
        assert!(
            matches!(record.write(&mut buf), Err(FormatError::Corrupt { .. })),
            "{why}"
        );
    }
    assert_eq!(
        IndexRecord::new("a", "u", 4096).write(&mut buf),
        Ok(br#"{"name":"a","uuid":"u","number_of_shards":4096,"settings":{}}"#.len())
    );
}

#[test]
fn tombstones_write_as_the_java_and_read_back() {
    let mut buf = [0u8; MAX_TOMBSTONE_BYTES];
    for (tombstone, text) in [
        (
            Tombstone {
                deleted_at: 1_763_164_800_123,
                reaping_at: 0,
            },
            r#"{"tombstone":true,"deleted_at":1763164800123}"#,
        ),
        (
            Tombstone {
                deleted_at: 5,
                reaping_at: 70_000,
            },
            r#"{"tombstone":true,"deleted_at":5,"reaping_at":70000}"#,
        ),
        (
            Tombstone {
                deleted_at: i64::MIN,
                reaping_at: i64::MIN,
            },
            r#"{"tombstone":true,"deleted_at":-9223372036854775808,"reaping_at":-9223372036854775808}"#,
        ),
    ] {
        let n = tombstone.write(&mut buf).unwrap();
        assert_eq!(std::str::from_utf8(&buf[..n]).unwrap(), text);
        assert_eq!(Tombstone::parse(&buf[..n]).unwrap(), tombstone);
    }
    assert_eq!(
        r#"{"tombstone":true,"deleted_at":-9223372036854775808,"reaping_at":-9223372036854775808}"#
            .len(),
        MAX_TOMBSTONE_BYTES,
        "the bound holds the longest"
    );
    // One written before times were recorded, and unknown fields.
    assert_eq!(
        Tombstone::parse(br#"{"tombstone":true}"#).unwrap(),
        Tombstone {
            deleted_at: 0,
            reaping_at: 0
        }
    );
    assert_eq!(
        Tombstone::parse(br#"{"tombstone":true,"x":[1],"reaping_at":9}"#).unwrap(),
        Tombstone {
            deleted_at: 0,
            reaping_at: 9
        }
    );
    for bad in [
        r#"{"tombstone":false}"#,
        r#"{"tombstone":true,"deleted_at":"5"}"#,
        r#"{"tombstone":true,"deleted_at":5.5}"#,
        r#"{"tombstone":true,"deleted_at":5,"deleted_at":6}"#,
        r#"{"tombstone":true,"reaping_at":5,"reaping_at":6}"#,
        r#"{"tombstone":true,"deleted_at":5"#,
        r#"{"tombstone":true}x"#,
    ] {
        assert!(
            matches!(
                Tombstone::parse(bad.as_bytes()),
                Err(FormatError::Corrupt { .. })
            ),
            "{bad}"
        );
    }
    let mut short = [0u8; 10];
    assert!(matches!(
        Tombstone {
            deleted_at: 1,
            reaping_at: 0
        }
        .write(&mut short),
        Err(FormatError::TooLarge { actual: 33, .. })
    ));
}

#[test]
fn a_delete_files_under_its_utc_hour() {
    let hour = 3_600_000i64;
    for (millis, bucket) in [
        (0, "1970010100"),
        (hour - 1, "1970010100"),
        (hour, "1970010101"),
        // 2024-02-29T23:59:59.999Z, then the next millisecond.
        (1_709_251_199_999, "2024022923"),
        (1_709_251_200_000, "2024030100"),
        // 2000-02-29T12:00Z, the last day of a 400-year cycle.
        (951_825_600_000, "2000022912"),
        // 2000-12-31T13:00Z, a leap century's last day.
        (978_267_600_000, "2000123113"),
        // 2100-03-01T00:00Z, after a century that is not a leap year.
        (4_107_542_400_000, "2100030100"),
        // The last millisecond of 9999.
        (253_402_300_799_999, "9999123123"),
    ] {
        assert_eq!(
            std::str::from_utf8(&tombstone_bucket(millis).unwrap()).unwrap(),
            bucket,
            "{millis}"
        );
    }
    for millis in [-1, 253_402_300_800_000, i64::MAX, i64::MIN] {
        assert!(tombstone_bucket(millis).is_err(), "{millis}");
    }
    assert_eq!(
        tombstone_marker_key("base/", "2024022923", "logs").unwrap(),
        "base/tombstones/2024022923/logs"
    );
    for (base, bucket, name) in [
        ("base", "b", "n"),
        ("", "", "n"),
        ("", "a/b", "n"),
        ("", "b", ""),
        ("", "b", ".."),
    ] {
        assert!(
            tombstone_marker_key(base, bucket, name).is_err(),
            "{base} {bucket} {name}"
        );
    }
}

#[test]
fn a_bucket_name_gives_back_its_hour() {
    let hour = 3_600_000i64;
    for start in [
        0i64,
        949_363_200_000,
        4_105_123_200_000,
        253_397_030_400_000,
    ] {
        // Sixty days from each: through a leap day, a non-leap century's February, and 9999's end.
        for h in 0..(24 * 60) {
            let millis = start + h * hour;
            let name = tombstone_bucket(millis).unwrap();
            let name = std::str::from_utf8(&name).unwrap();
            assert_eq!(tombstone_bucket_start(name), Some(millis), "{name}");
        }
    }
    assert_eq!(
        tombstone_bucket_start("9999123123"),
        Some(253_402_297_200_000)
    );
    for bad in [
        "",
        "197001010",
        "19700101000",
        "1970010100x",
        "19700101 0",
        "1969123123",
        "1970130100",
        "1970000100",
        "1970010000",
        "1970043100",
        "1970022900",
        "1900022900",
        "2100022900",
        "2023022900",
        "1970010124",
        "１９７００１０１００",
    ] {
        assert_eq!(tombstone_bucket_start(bad), None, "{bad}");
    }
    assert_eq!(tombstone_bucket_start("2000022900"), Some(951_782_400_000));
    assert_eq!(
        tombstone_bucket_start("2024022923"),
        Some(1_709_247_600_000)
    );
}
