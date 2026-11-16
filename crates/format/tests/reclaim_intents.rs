//! Reclaim intents read and written as the Java's `ReclaimQueue` does, and
//! the minute buckets they fall due in.

use std::borrow::Cow;

use oss_format::{
    FormatError, MAX_RECLAIM_INTENT_BYTES, RECLAIM_PREFIX, ReclaimIntent, reclaim_bucket,
    reclaim_bucket_start, reclaim_intent_key,
};

fn intent(had_writer: bool) -> ReclaimIntent<'static> {
    ReclaimIntent {
        name: Cow::Borrowed("logs-2026"),
        uuid: Cow::Borrowed("Zq3_uuid-AB"),
        shards: 12,
        had_writer,
    }
}

fn written(intent: &ReclaimIntent<'_>) -> String {
    let mut buf = [0u8; MAX_RECLAIM_INTENT_BYTES];
    let len = intent.write(&mut buf).unwrap();
    String::from_utf8(buf[..len].to_vec()).unwrap()
}

fn corrupt(value: &str) -> bool {
    matches!(
        ReclaimIntent::parse(value.as_bytes()),
        Err(FormatError::Corrupt { .. })
    )
}

#[test]
fn an_intent_is_written_byte_for_byte_as_the_java_concatenates_it() {
    // The Java's enqueue: "{\"name\":\"" + name + "\",\"uuid\":\"" + uuid
    // + "\",\"shards\":" + shards + ",\"had_writer\":" + hadWriter + "}".
    assert_eq!(
        written(&intent(true)),
        r#"{"name":"logs-2026","uuid":"Zq3_uuid-AB","shards":12,"had_writer":true}"#
    );
    assert_eq!(
        written(&intent(false)),
        r#"{"name":"logs-2026","uuid":"Zq3_uuid-AB","shards":12,"had_writer":false}"#
    );
}

#[test]
fn a_written_intent_reads_back_and_a_short_buffer_says_what_it_needs() {
    for had_writer in [true, false] {
        let mut buf = [0u8; 128];
        let len = intent(had_writer).write(&mut buf).unwrap();
        assert_eq!(
            ReclaimIntent::parse(&buf[..len]).unwrap(),
            intent(had_writer)
        );
        let mut short = vec![0u8; len - 1];
        assert!(matches!(
            intent(had_writer).write(&mut short),
            Err(FormatError::TooLarge { actual, .. }) if actual == len as u64
        ));
    }
}

#[test]
fn had_writer_is_false_only_for_the_literal_false() {
    let with = |value: &str| {
        let body = format!(r#"{{"name":"a","uuid":"u","shards":1{value}}}"#);
        ReclaimIntent::parse(body.as_bytes()).unwrap().had_writer
    };
    assert!(!with(r#","had_writer":false"#));
    assert!(with(r#","had_writer":true"#));
    // Absent, or anything but false, reads as true, as Boolean.FALSE.equals.
    assert!(with(""));
    assert!(with(r#","had_writer":"false""#));
    assert!(with(r#","had_writer":0"#));
    assert!(with(r#","had_writer":null"#));
    assert!(with(r#","had_writer":{"x":[false]}"#));
    assert!(with(r#","had_writer":[false]"#));
}

#[test]
fn unknown_fields_are_skipped_and_order_does_not_matter() {
    let parsed = ReclaimIntent::parse(
        br#"{"extra":{"n":[1,2]},"shards":0,"had_writer":false,"uuid":"uA","name":"n"}"#,
    )
    .unwrap();
    assert_eq!(parsed.name, "n");
    assert_eq!(parsed.uuid, "uA");
    assert_eq!(parsed.shards, 0);
    assert!(!parsed.had_writer);
}

#[test]
fn an_intent_missing_or_mistyping_a_field_is_corrupt() {
    for body in [
        r#"{"uuid":"u","shards":1}"#,
        r#"{"name":"n","shards":1}"#,
        r#"{"name":"n","uuid":"u"}"#,
        r#"{"name":1,"uuid":"u","shards":1}"#,
        r#"{"name":"n","uuid":null,"shards":1}"#,
        r#"{"name":"n","uuid":"u","shards":"1"}"#,
        r#"{"name":"n","uuid":"u","shards":1.5}"#,
        r#"{"name":"n","uuid":"u","shards":-1}"#,
        r#"{"name":"n","uuid":"u","shards":4097}"#,
        r#"{"name":"n","uuid":"u","shards":99999999999}"#,
        r#"["name","n"]"#,
        "",
    ] {
        assert!(corrupt(body), "{body}");
    }
    assert_eq!(
        ReclaimIntent::parse(br#"{"name":"n","uuid":"u","shards":4096}"#)
            .unwrap()
            .shards,
        4096
    );
}

#[test]
fn a_repeated_field_is_corrupt_as_jackson_strict_detection_refuses_it() {
    for body in [
        r#"{"name":"n","name":"m","uuid":"u","shards":1}"#,
        r#"{"name":"n","uuid":"u","uuid":"v","shards":1}"#,
        r#"{"name":"n","uuid":"u","shards":1,"shards":2}"#,
        r#"{"name":"n","uuid":"u","shards":1,"had_writer":true,"had_writer":false}"#,
    ] {
        assert!(corrupt(body), "{body}");
    }
}

#[test]
fn a_truncated_or_trailing_intent_is_corrupt() {
    let full = written(&intent(true));
    for end in 0..full.len() {
        assert!(
            ReclaimIntent::parse(&full.as_bytes()[..end]).is_err(),
            "{end}"
        );
    }
    assert!(ReclaimIntent::parse(format!("{full} {{}}").as_bytes()).is_err());
}

#[test]
fn a_bucket_is_the_utc_minute_a_due_time_falls_in() {
    let at = |millis| String::from_utf8(reclaim_bucket(millis).unwrap().to_vec()).unwrap();
    assert_eq!(at(0), "197001010000");
    assert_eq!(at(59_999), "197001010000");
    assert_eq!(at(60_000), "197001010001");
    // 2026-11-16T13:47:05.123Z
    assert_eq!(at(1_794_836_825_123), "202611161347");
    // 2024-02-29T23:59:59.999Z
    assert_eq!(at(1_709_251_199_999), "202402292359");
    assert_eq!(at(253_402_300_799_999), "999912312359");
    for millis in [-1, 253_402_300_800_000, i64::MIN, i64::MAX] {
        assert!(matches!(
            reclaim_bucket(millis),
            Err(FormatError::Corrupt { .. })
        ));
    }
}

#[test]
fn a_bucket_starts_at_its_minute_and_round_trips() {
    for millis in [
        0,
        60_000,
        1_794_836_825_123,
        1_709_251_199_999,
        253_402_300_799_999,
    ] {
        let bucket = reclaim_bucket(millis).unwrap();
        let start = reclaim_bucket_start(std::str::from_utf8(&bucket).unwrap()).unwrap();
        assert_eq!(start, millis - millis % 60_000);
    }
    assert_eq!(
        reclaim_bucket_start("202611161347"),
        Some(1_794_836_820_000)
    );
}

#[test]
fn a_name_that_is_not_a_real_minute_has_no_start() {
    for name in [
        "",
        "20261116134",
        "2026111613470",
        "2026111613a7",
        "+02611161347",
        "196912312359",
        "202600161347",
        "202613161347",
        "202611001347",
        "202611311347",
        "202302291200",
        "202611162447",
        "202611161360",
    ] {
        assert_eq!(reclaim_bucket_start(name), None, "{name}");
    }
    assert_eq!(
        reclaim_bucket_start("202402291200"),
        Some(1_709_208_000_000)
    );
    assert_eq!(
        reclaim_bucket_start("202611302359"),
        Some(1_796_083_140_000)
    );
}

#[test]
fn an_intent_key_is_filed_under_its_bucket_by_name_and_uuid() {
    assert_eq!(RECLAIM_PREFIX, "reclaim/");
    assert_eq!(
        reclaim_intent_key("", "202611161347", "logs", "u1").unwrap(),
        "reclaim/202611161347/logs#u1"
    );
    assert_eq!(
        reclaim_intent_key("d/", "202611161347", "logs", "u1").unwrap(),
        "d/reclaim/202611161347/logs#u1"
    );
    for (base, bucket, index, uuid) in [
        ("d", "b", "i", "u"),
        ("", "", "i", "u"),
        ("", "a/b", "i", "u"),
        ("", "b", "", "u"),
        ("", "b", "i/j", "u"),
        ("", "b", "i#j", "u"),
        ("", "b", "i", "u#v"),
        ("", "b", "i", ".."),
    ] {
        assert!(
            reclaim_intent_key(base, bucket, index, uuid).is_err(),
            "{base} {bucket} {index} {uuid}"
        );
    }
}
