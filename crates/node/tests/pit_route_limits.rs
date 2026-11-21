//! docs/specs/node-pit-route: the limits, units and request counts of the
//! point-in-time routes.

#[path = "pit_support/mod.rs"]
mod pit_support;

use oss_blob::S3Op;
use pit_support::{BUF, call, create, id_of, index, rig, rig_sized, sent};

#[test]
fn requests_are_what_the_spec_says_and_a_refusal_sends_none() {
    let r = rig();
    index(&r, "logs", 2);
    let (_, made) = sent(&r, || {
        call(
            &r,
            "POST",
            "/logs/_search/point_in_time?keep_alive=2h",
            None,
        )
    });
    assert!(
        made.is_empty(),
        "a refused keep-alive sends nothing: {made:?}"
    );
    let (reply, made) = sent(&r, || call(&r, "POST", "/logs/_search/point_in_time", None));
    assert_eq!(reply.status, 200);
    assert_eq!(
        made,
        [(S3Op::Get, 5), (S3Op::Head, 1), (S3Op::Put, 3)],
        "two shards: the capture's budget"
    );
    let view = id_of(&reply.text());
    let second = create(&r, "logs");
    let (_, made) = sent(&r, || call(&r, "GET", "/_search/point_in_time/_all", None));
    assert_eq!(
        made,
        [(S3Op::Get, 2), (S3Op::List, 1)],
        "one LIST page and a read a view"
    );
    let (_, made) = sent(&r, || call(&r, "DELETE", &format!("/_pit/{view}"), None));
    assert_eq!(
        made,
        [(S3Op::Get, 1), (S3Op::Put, 1), (S3Op::Delete, 1)],
        "a read, the release swap and the marker's delete"
    );
    let (_, made) = sent(&r, || call(&r, "DELETE", &format!("/_pit/{second}"), None));
    assert_eq!(made, [(S3Op::Get, 1), (S3Op::Put, 1), (S3Op::Delete, 1)]);
    let (_, made) = sent(&r, || call(&r, "GET", "/_search/point_in_time/_all", None));
    assert_eq!(made, [(S3Op::List, 1)], "nothing live, nothing to read");
}

#[test]
fn time_values_take_every_unit_and_overflow_is_refused() {
    let r = rig();
    index(&r, "logs", 1);
    for (value, millis) in [
        ("0", 0u64),
        ("1000000nanos", 1),
        ("2000micros", 2),
        ("2500000nanos", 2),
        ("3s", 3_000),
        ("1h", 3_600_000),
    ] {
        let text = call(
            &r,
            "POST",
            &format!("/logs/_search/point_in_time?keep_alive={value}"),
            None,
        )
        .text();
        assert!(
            text.contains(&format!(r#""keep_alive_millis":{millis}"#)),
            "{value}: {text}"
        );
    }
    // A day is past the cap, and says so: the parse took it as 86,400,000.
    let day = call(
        &r,
        "POST",
        "/logs/_search/point_in_time?keep_alive=1d",
        None,
    );
    assert!(day.text().contains("keep_alive_too_long"), "{}", day.text());
    let huge = call(
        &r,
        "POST",
        "/logs/_search/point_in_time?keep_alive=99999999999999999d",
        None,
    );
    assert_eq!(huge.status, 400, "{}", huge.text());
    assert!(huge.text().contains("illegal_argument_exception"));
    assert!(huge.text().contains("too large"), "{}", huge.text());
}

#[test]
fn empty_routing_and_preference_are_no_refusal_and_both_spellings_work() {
    let r = rig();
    index(&r, "logs", 1);
    let reply = call(&r, "POST", "/logs/_pit?routing=&preference=", None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    let id = id_of(&reply.text());
    let gone = call(&r, "DELETE", &format!("/_pit/{id}"), None);
    assert_eq!(gone.status, 200, "{}", gone.text());
}

#[test]
fn a_delete_body_is_bounded_deduplicated_and_strict() {
    let r = rig();
    index(&r, "logs", 1);
    let view = create(&r, "logs");
    let twice = format!(r#"{{"pit_id":["{view}","{view}"]}}"#);
    let reply = call(&r, "DELETE", "/_search/point_in_time", Some(&twice));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        reply.text(),
        format!(r#"{{"pits":[{{"successful":true,"pit_id":"{view}"}}]}}"#),
        "one entry an id, released once"
    );
    for (body, why) in [
        (r#"{"pit_id":[]}"#, "an empty array names nothing"),
        (r#"{"pit_id":"a","pit_id":"b"}"#, "twice"),
    ] {
        let reply = call(&r, "DELETE", "/_search/point_in_time", Some(body));
        assert_eq!(reply.status, 400, "{why}: {}", reply.text());
    }
    // _all among other ids releases every finished view.
    let one = create(&r, "logs");
    let mixed = r#"{"pit_id":["AAAAAAAAAAAAAAAAAAAAAA","_all"]}"#;
    let reply = call(&r, "DELETE", "/_search/point_in_time", Some(mixed));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(reply.text().contains(&one), "{}", reply.text());
    // The bounds: 1,024 ids pass the parse (none exist: 404), 1,025 are refused.
    let ids = |n: usize| {
        let list: Vec<String> = (0..n).map(|i| format!("\"{i:022}\"")).collect();
        format!(r#"{{"pit_id":[{}]}}"#, list.join(","))
    };
    let at = call(&r, "DELETE", "/_search/point_in_time", Some(&ids(1024)));
    assert_eq!(at.status, 404, "{}", at.text());
    let over = call(&r, "DELETE", "/_search/point_in_time", Some(&ids(1025)));
    assert_eq!(over.status, 400, "{}", over.text());
    let long = format!(r#"{{"pit_id":"{}"}}"#, "x".repeat(256));
    assert_eq!(
        call(&r, "DELETE", "/_search/point_in_time", Some(&long)).status,
        404
    );
    let longer = format!(r#"{{"pit_id":"{}"}}"#, "x".repeat(257));
    assert_eq!(
        call(&r, "DELETE", "/_search/point_in_time", Some(&longer)).status,
        400
    );
}

#[test]
fn a_request_the_body_pool_cannot_hold_a_buffer_for_is_refused_with_429() {
    // The rig's request-body pool is 16 MiB.
    let r = rig_sized(64 << 20);
    index(&r, "logs", 1);
    let reply = call(&r, "POST", "/logs/_search/point_in_time", None);
    assert_eq!(reply.status, 429, "{}", reply.text());
    assert!(r.pits.live(&mut vec![0u8; BUF]).expect("live").is_empty());
}

#[test]
fn a_view_that_does_not_fit_the_buffer_fails_cleanly_and_pins_nothing() {
    let r = rig_sized(200);
    index(&r, "logs", 2);
    let reply = call(&r, "POST", "/logs/_search/point_in_time", None);
    assert_eq!(reply.status, 500, "{}", reply.text());
    assert!(
        reply.text().contains("point_in_time_failure"),
        "{}",
        reply.text()
    );
    assert!(r.pits.live(&mut vec![0u8; BUF]).expect("live").is_empty());
}
