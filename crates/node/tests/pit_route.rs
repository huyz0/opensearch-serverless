//! docs/specs/node-pit-route: points in time over REST.

#[path = "pit_support/mod.rs"]
mod pit_support;

use std::sync::atomic::Ordering;

use oss_format::{IndexRecord, PointInTime};
use pit_support::{BUF, call, create, id_of, index, rig};

#[test]
fn a_create_freezes_every_shard_and_answers_the_view() {
    let r = rig();
    index(&r, "logs", 2);
    for target in [
        "/logs/_search/point_in_time?keep_alive=2m",
        "/logs/_pit?keep_alive=2m",
    ] {
        let reply = call(&r, "POST", target, None);
        let text = reply.text();
        assert_eq!(reply.status, 200, "{text}");
        assert!(
            text.contains(r#""_shards":{"total":2,"successful":2,"skipped":0,"failed":0}"#),
            "{text}"
        );
        assert!(text.contains(r#""creation_time":1000000"#), "{text}");
        assert!(text.contains(r#""index":"logs""#), "{text}");
        assert!(text.contains(r#""keep_alive_millis":120000"#), "{text}");
        let view = r
            .pits
            .read(&id_of(&text), &mut vec![0u8; BUF])
            .expect("read")
            .expect("a finished view");
        assert_eq!(view.shards.len(), 2);
        assert_eq!(view.expires_at_millis, 1_000_000 + 120_000);
    }
}

#[test]
fn keep_alive_takes_time_units_and_is_refused_past_an_hour() {
    let r = rig();
    index(&r, "logs", 1);
    for (value, millis) in [
        ("500ms", 500u64),
        ("30s", 30_000),
        ("2m", 120_000),
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
    let live = r.pits.live(&mut vec![0u8; BUF]).expect("live").len();
    for value in ["1d", "61m", "3600001ms"] {
        let reply = call(
            &r,
            "POST",
            &format!("/logs/_search/point_in_time?keep_alive={value}"),
            None,
        );
        assert_eq!(reply.status, 400, "{value}: {}", reply.text());
        assert!(reply.text().contains("keep_alive_too_long"), "{value}");
    }
    for value in ["abc", "5", "m", "-"] {
        let reply = call(
            &r,
            "POST",
            &format!("/logs/_search/point_in_time?keep_alive={value}"),
            None,
        );
        assert_eq!(reply.status, 400, "{value}: {}", reply.text());
        assert!(
            reply.text().contains("illegal_argument_exception"),
            "{value}"
        );
    }
    assert_eq!(
        r.pits.live(&mut vec![0u8; BUF]).expect("live").len(),
        live,
        "a refused keep-alive writes nothing"
    );
}

#[test]
fn what_cannot_be_frozen_is_refused_with_the_javas_status() {
    let r = rig();
    index(&r, "logs", 2);
    r.descriptors
        .create_alias(
            &oss_meta::AliasRecord::new("a", &["logs"]),
            1,
            &mut vec![0u8; BUF],
        )
        .expect("alias");
    r.descriptors
        .create(&IndexRecord::new("empty", "u3", 1), 1, &mut vec![0u8; BUF])
        .expect("created");
    for (target, status, kind) in [
        (
            "/logs,other/_search/point_in_time",
            501,
            "unsupported_point_in_time",
        ),
        (
            "/lo*/_search/point_in_time",
            501,
            "unsupported_point_in_time",
        ),
        ("/a/_search/point_in_time", 501, "unsupported_point_in_time"),
        (
            "/logs/_search/point_in_time?routing=x",
            501,
            "unsupported_point_in_time",
        ),
        (
            "/logs/_search/point_in_time?preference=_local",
            501,
            "unsupported_point_in_time",
        ),
        ("/absent/_search/point_in_time", 404, "index_not_found"),
        ("/empty/_search/point_in_time", 409, "nothing_published"),
    ] {
        let reply = call(&r, "POST", target, None);
        assert_eq!(reply.status, status, "{target}: {}", reply.text());
        assert!(reply.text().contains(kind), "{target}: {}", reply.text());
    }
    assert!(
        r.pits.live(&mut vec![0u8; BUF]).expect("live").is_empty(),
        "nothing stays pinned"
    );
}

#[test]
fn a_delete_by_path_body_and_all_releases_and_says_which() {
    let r = rig();
    index(&r, "logs", 1);
    let (a, b, c, d) = (
        create(&r, "logs"),
        create(&r, "logs"),
        create(&r, "logs"),
        create(&r, "logs"),
    );
    let by_path = call(&r, "DELETE", &format!("/_search/point_in_time/{a}"), None);
    assert_eq!(by_path.status, 200, "{}", by_path.text());
    assert_eq!(
        by_path.text(),
        format!(r#"{{"pits":[{{"successful":true,"pit_id":"{a}"}}]}}"#)
    );
    let by_pit = call(&r, "DELETE", &format!("/_pit/{b}"), None);
    assert_eq!(by_pit.status, 200, "{}", by_pit.text());
    let gone = call(&r, "DELETE", &format!("/_search/point_in_time/{a}"), None);
    assert_eq!(gone.status, 404, "{}", gone.text());
    assert!(gone.text().contains("pit_not_found"));
    let body = format!(r#"{{"pit_id":["{c}","AAAAAAAAAAAAAAAAAAAAAA"]}}"#);
    let by_body = call(&r, "DELETE", "/_search/point_in_time", Some(&body));
    assert_eq!(by_body.status, 200, "{}", by_body.text());
    let text = by_body.text();
    assert!(
        text.contains(&format!(r#"{{"successful":true,"pit_id":"{c}"}}"#)),
        "{text}"
    );
    assert!(
        text.contains(r#"{"successful":false,"pit_id":"AAAAAAAAAAAAAAAAAAAAAA"}"#),
        "{text}"
    );
    let single = format!(r#"{{"pit_id":"{d}"}}"#);
    let one = call(&r, "DELETE", "/_search/point_in_time", Some(&single));
    assert_eq!(one.status, 200, "{}", one.text());
    assert!(
        r.pits.live(&mut vec![0u8; BUF]).expect("live").is_empty(),
        "every view was released"
    );
    let e = create(&r, "logs");
    let f = create(&r, "logs");
    let all = call(&r, "DELETE", "/_search/point_in_time/_all", None);
    assert_eq!(all.status, 200, "{}", all.text());
    assert!(
        all.text().contains(&e) && all.text().contains(&f),
        "{}",
        all.text()
    );
    let all_body = call(
        &r,
        "DELETE",
        "/_search/point_in_time",
        Some(r#"{"pit_id":["_all"]}"#),
    );
    assert_eq!(
        all_body.status,
        200,
        "an empty _all is not an error: {}",
        all_body.text()
    );
    assert_eq!(all_body.text(), r#"{"pits":[]}"#);
}

#[test]
fn a_delete_that_names_nothing_usable_is_refused_not_failed() {
    let r = rig();
    for (body, status) in [
        (None, 400),
        (Some("{}"), 400),
        (Some(r#"{"other":["x"]}"#), 400),
        (Some(r#"{"pit_id":[1]}"#), 400),
        (Some("{not json"), 400),
        (Some(r#"{"pit_id":"not an id!"}"#), 404),
    ] {
        let reply = call(&r, "DELETE", "/_search/point_in_time", body);
        assert_eq!(reply.status, status, "{body:?}: {}", reply.text());
    }
}

#[test]
fn all_leaves_a_capture_in_progress_and_the_listing_hides_it() {
    let r = rig();
    index(&r, "logs", 1);
    let view = create(&r, "logs");
    let capturing = PointInTime {
        id: "BBBBBBBBBBBBBBBBBBBBBB".to_owned(),
        index: "logs".to_owned(),
        index_uuid: Some("u1".to_owned()),
        expires_at_millis: 1_000_000 + 60_000,
        capturing: true,
        shards: Vec::new(),
    };
    r.pits
        .create(&capturing, &mut vec![0u8; BUF])
        .expect("marker");
    let listed = call(&r, "GET", "/_search/point_in_time/_all", None);
    assert_eq!(listed.status, 200, "{}", listed.text());
    assert!(listed.text().contains(&view), "{}", listed.text());
    assert!(!listed.text().contains("BBBB"), "{}", listed.text());
    let all = call(&r, "DELETE", "/_search/point_in_time/_all", None);
    assert!(
        all.text().contains(&view) && !all.text().contains("BBBB"),
        "{}",
        all.text()
    );
    let live = r.pits.live(&mut vec![0u8; BUF]).expect("live");
    assert_eq!(live.len(), 1, "the capture is still pinned");
    assert!(live[0].capturing);
}

#[test]
fn a_list_gives_each_finished_view_its_remaining_life_and_drops_the_expired() {
    let r = rig();
    index(&r, "logs", 1);
    index(&r, "docs", 1);
    let short = call(
        &r,
        "POST",
        "/logs/_search/point_in_time?keep_alive=1m",
        None,
    )
    .text();
    let long = call(
        &r,
        "POST",
        "/docs/_search/point_in_time?keep_alive=10m",
        None,
    )
    .text();
    r.now.fetch_add(30_000, Ordering::SeqCst);
    let text = call(&r, "GET", "/_search/point_in_time/_all", None).text();
    assert!(
        text.contains(&format!(
            r#"{{"pit_id":"{}","keep_alive":30000,"index":"logs"}}"#,
            id_of(&short)
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            r#"{{"pit_id":"{}","keep_alive":570000,"index":"docs"}}"#,
            id_of(&long)
        )),
        "{text}"
    );
    r.now.fetch_add(60_000, Ordering::SeqCst);
    let later = call(&r, "GET", "/_search/point_in_time/_all", None).text();
    assert!(!later.contains(&id_of(&short)), "{later}");
    assert!(later.contains(&id_of(&long)), "{later}");
    r.now.fetch_add(3_600_000, Ordering::SeqCst);
    assert_eq!(
        call(&r, "GET", "/_search/point_in_time/_all", None).text(),
        r#"{"pits":[]}"#
    );
}

#[test]
fn the_literal_all_wins_over_an_id_in_the_path() {
    let r = rig();
    index(&r, "logs", 1);
    let view = create(&r, "logs");
    // `_all` as an id would be 404 `pit_not_found`; as the literal it lists.
    assert_eq!(
        call(&r, "GET", "/_search/point_in_time/_all", None).status,
        200
    );
    assert_eq!(
        call(&r, "DELETE", "/_search/point_in_time/_all", None).status,
        200
    );
    assert!(
        !r.pits
            .live(&mut vec![0u8; BUF])
            .expect("live")
            .iter()
            .any(|v| v.id == view)
    );
}

#[test]
fn a_release_tells_the_hook_each_view_it_removed_and_no_other() {
    use std::sync::{Arc, Mutex};
    let r = rig();
    index(&r, "logs", 1);
    let told: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&told);
    assert!(r.service.set_release_hook(Arc::new(move |id| {
        sink.lock().expect("lock").push(id.to_owned())
    })));
    assert!(!r.service.set_release_hook(Arc::new(|_| {})), "one hook");
    let a = create(&r, "logs");
    let b = create(&r, "logs");
    let c = create(&r, "logs");
    call(&r, "DELETE", &format!("/_search/point_in_time/{a}"), None);
    // A repeat of a released id and an id that was never a view are not told.
    call(&r, "DELETE", &format!("/_search/point_in_time/{a}"), None);
    call(&r, "DELETE", "/_search/point_in_time/not-an-id", None);
    assert_eq!(*told.lock().expect("lock"), vec![a.clone()]);
    call(&r, "DELETE", "/_search/point_in_time/_all", None);
    let mut all = told.lock().expect("lock").clone();
    all.sort();
    let mut want = vec![a, b, c];
    want.sort();
    assert_eq!(all, want);
}
