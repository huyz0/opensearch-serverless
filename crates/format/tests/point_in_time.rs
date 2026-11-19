//! docs/specs/point-in-time-store/SPEC.md, AC-1 to AC-3: the point-in-time record.

use oss_format::manifest_format::MAX_FILES;
use oss_format::{
    FormatError, MAX_POINT_IN_TIME_BYTES, PitFile, PitShard, PointInTime, RELEASED,
    looks_like_an_id, point_in_time_key, point_in_time_state_key,
};

/// What the Java's `PointInTime.toBytes` writes for a view of one shard with a
/// whole file, a packed one and a recorded length for each.
const JAVA: &str = concat!(
    r#"{"id":"AAAAAAAAAAAAAAAAAAAAAA","index":"logs","index_uuid":"u-1","expires_at":1700000000000,"#,
    r#""shards":[{"shard":0,"term":3,"files":{"_0.cfe":"t=3/s1#0:40","_0.si":"t=3"},"#,
    r#""lengths":{"_0.cfe":40,"_0.si":7}}]}"#
);

fn view() -> PointInTime {
    PointInTime {
        id: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
        index: "logs".to_owned(),
        index_uuid: Some("u-1".to_owned()),
        expires_at_millis: 1_700_000_000_000,
        capturing: false,
        shards: vec![PitShard {
            shard: 0,
            term: 3,
            files: vec![
                PitFile {
                    name: "_0.cfe".to_owned(),
                    location: "t=3/s1#0:40".to_owned(),
                    length: Some(40),
                },
                PitFile {
                    name: "_0.si".to_owned(),
                    location: "t=3".to_owned(),
                    length: Some(7),
                },
            ],
        }],
    }
}

fn written(pit: &PointInTime) -> String {
    let mut buf = vec![0u8; 4096];
    let n = pit.write(&mut buf).expect("fits");
    String::from_utf8(buf[..n].to_vec()).expect("utf8")
}

#[test]
fn bytes_the_java_writes_are_read_and_written_back_the_same() {
    assert_eq!(PointInTime::parse(JAVA.as_bytes()).expect("parses"), view());
    assert_eq!(written(&view()), JAVA);
}

#[test]
fn a_record_without_a_uuid_or_lengths_reads_and_writes_back() {
    let old = r#"{"id":"a","index":"i","expires_at":5,"shards":[{"shard":2,"term":0,"files":{"f":"t=0"}}]}"#;
    let pit = PointInTime::parse(old.as_bytes()).expect("parses");
    assert_eq!(pit.index_uuid, None);
    assert_eq!(
        pit.shards
            .first()
            .and_then(|s| s.files.first())
            .map(|f| f.length),
        Some(None)
    );
    assert_eq!(written(&pit), old);
}

#[test]
fn a_capture_marker_holds_no_shards_and_says_so() {
    let marker = view().marker();
    let text = written(&marker);
    assert_eq!(
        text,
        r#"{"id":"AAAAAAAAAAAAAAAAAAAAAA","index":"logs","index_uuid":"u-1","expires_at":9223372036854775807,"capturing":true,"shards":[]}"#
    );
    assert_eq!(PointInTime::parse(text.as_bytes()).expect("parses"), marker);
}

#[test]
fn views_and_markers_are_told_apart_by_their_shards() {
    let finished_empty = r#"{"id":"a","index":"i","expires_at":5,"shards":[]}"#;
    let marker_with_shards = JAVA.replace(
        r#""expires_at":1700000000000,"#,
        r#""expires_at":5,"capturing":true,"#,
    );
    for bad in [finished_empty.to_owned(), marker_with_shards] {
        assert!(
            matches!(
                PointInTime::parse(bad.as_bytes()),
                Err(FormatError::Corrupt { .. })
            ),
            "{bad}"
        );
    }
}

#[test]
fn a_view_pins_its_index_by_uuid_when_both_know_one_and_by_name_otherwise() {
    let pit = view();
    assert!(pit.pins("logs", Some("u-1")));
    assert!(!pit.pins("logs", Some("u-2")), "another incarnation");
    assert!(
        pit.pins("logs", None),
        "an unknown uuid errs towards pinning"
    );
    assert!(!pit.pins("other", Some("u-1")));
    let old = PointInTime {
        index_uuid: None,
        ..view()
    };
    assert!(
        old.pins("logs", Some("u-9")),
        "a record without a uuid matches by name"
    );
}

#[test]
fn a_view_expires_at_its_deadline_and_extends_keeping_everything_else() {
    let pit = view();
    assert!(!pit.expired_at(1_699_999_999_999));
    assert!(pit.expired_at(1_700_000_000_000));
    let later = pit.clone().with_expiry(1_800_000_000_000);
    assert_eq!(later.expires_at_millis, 1_800_000_000_000);
    assert_eq!(later.index_uuid, pit.index_uuid);
    assert_eq!(later.shards, pit.shards);
}

#[test]
fn malformed_records_are_refused() {
    let cases = [
        "",
        "[]",
        r#"{"id":"a"}"#,
        &JAVA.replace(r#""id":"AAAAAAAAAAAAAAAAAAAAAA","#, r#""id":"","#),
        &JAVA.replace(r#""expires_at":1700000000000"#, r#""expires_at":-1"#),
        &JAVA.replace(r#""expires_at":1700000000000"#, r#""expires_at":"5""#),
        &JAVA.replace(r#""id":"AAAAAAAAAAAAAAAAAAAAAA","#, r#""id":"a","id":"b","#),
        &JAVA.replace(r#""shard":0"#, r#""shard":4096"#),
        &JAVA.replace(r#""term":3"#, r#""term":-3"#),
        &JAVA.replace(r#""_0.si":7"#, r#""_0.zz":7"#),
        &JAVA.replace(r#""_0.si":7"#, r#""_0.si":-7"#),
        &JAVA.replace(r#""_0.si":"t=3""#, r#""_0.si":"""#),
        &JAVA.replace(r#""files":{"#, r#""files":[],"x":{"#),
        &format!("{JAVA} x"),
        &JAVA.replace(
            r#""shards":[{"shard":0,"term":3,"files":{"_0.cfe""#,
            r#""shards":[{"shard":0,"term":3,"files":{"_0.cfe":"t=3","_0.cfe""#,
        ),
    ];
    for bad in cases {
        assert!(PointInTime::parse(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn a_shard_given_twice_is_refused() {
    let one = r#"{"shard":1,"term":0,"files":{"f":"t=0"}}"#;
    let twice = format!(r#"{{"id":"a","index":"i","expires_at":5,"shards":[{one},{one}]}}"#);
    assert!(PointInTime::parse(twice.as_bytes()).is_err());
}

#[test]
fn a_record_past_the_bound_is_refused_before_it_is_read() {
    let huge = vec![b' '; MAX_POINT_IN_TIME_BYTES + 1];
    assert!(matches!(
        PointInTime::parse(&huge),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_shard_past_the_file_bound_is_refused() {
    let mut text = String::from(
        r#"{"id":"a","index":"i","expires_at":5,"shards":[{"shard":0,"term":0,"files":{"#,
    );
    for n in 0..=MAX_FILES {
        if n > 0 {
            text.push(',');
        }
        text.push_str(&format!(r#""f{n}":"t=0""#));
    }
    text.push_str("}}]}");
    assert!(matches!(
        PointInTime::parse(text.as_bytes()),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_short_buffer_reports_the_length_needed() {
    let mut buf = [0u8; 8];
    let err = view().write(&mut buf).expect_err("too short");
    assert!(matches!(err, FormatError::TooLarge { actual, .. } if actual == JAVA.len() as u64));
}

#[test]
fn a_deadline_past_a_signed_integer_is_not_written() {
    let mut pit = view();
    pit.expires_at_millis = u64::MAX;
    assert!(pit.write(&mut [0u8; 4096]).is_err());
}

#[test]
fn the_released_value_is_not_a_view() {
    assert!(PointInTime::parse(RELEASED).is_err());
}

#[test]
fn keys_are_derived_from_a_base_and_an_id() {
    let id = "AAAAAAAAAAAAAAAAAAAAAA";
    assert_eq!(
        point_in_time_key("", id).expect("key"),
        format!("pits/{id}")
    );
    assert_eq!(
        point_in_time_state_key("b/", id).expect("key"),
        format!("b/pit-states/{id}")
    );
    assert!(
        point_in_time_key("b", id).is_err(),
        "a base ends in a slash"
    );
    for bad in [
        "",
        "short",
        "AAAAAAAAAAAAAAAAAAAAA/",
        "AAAAAAAAAAAAAAAAAAAAAAA",
        "AAAAAAAAAAAAAAAAAAAAA.",
    ] {
        assert!(!looks_like_an_id(bad), "{bad}");
        assert!(point_in_time_key("", bad).is_err(), "{bad}");
    }
    assert!(looks_like_an_id("a-_Z09aAaAaAaAaAaAaAaA"));
}

#[test]
fn a_placeholder_names_no_index() {
    let mut pit = view();
    assert!(!pit.is_placeholder());
    pit.index = String::new();
    assert!(pit.is_placeholder());
}

#[test]
fn a_record_of_exactly_the_bound_is_read() {
    let mut text = JAVA.as_bytes().to_vec();
    text.resize(MAX_POINT_IN_TIME_BYTES, b' ');
    assert_eq!(PointInTime::parse(&text).expect("at the bound"), view());
    text.push(b' ');
    assert!(PointInTime::parse(&text).is_err());
}

#[test]
fn a_files_blob_key_is_derived_as_the_manifests_are() {
    use oss_format::{FileLocation, ShardRef};
    let shard = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 2,
    };
    let key = |name, location| shard.blob_key("d/", name, &location).expect("key");
    assert_eq!(
        key("_0.si", FileLocation::Whole { term_dir: "t=3" }),
        ("d/segments/logs#u1#2/t=3/_0.si".to_owned(), 0)
    );
    assert_eq!(
        key(
            "_0.cfs",
            FileLocation::Packed {
                term_dir: "t=3",
                blob: "s1",
                offset: 10,
                length: 5
            }
        ),
        ("d/segments/logs#u1#2/t=3/s1".to_owned(), 10)
    );
    assert_eq!(
        key(
            "_0.cfs",
            FileLocation::NodePacked {
                node: "n1",
                epoch: 3,
                ordinal: 7,
                offset: 4,
                length: 5
            }
        ),
        (
            "d/packs/n1/e=00000000000000000003/00000000000000000007".to_owned(),
            4
        )
    );
    assert!(
        shard
            .blob_key("d/", "a/b", &FileLocation::Whole { term_dir: "t=3" })
            .is_err()
    );
}
