//! docs/specs/snapshot-global-state: the record's `global_state` member.

// ARITH: test sizes are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use std::path::PathBuf;

use oss_format::{
    FormatError, GlobalState, MAX_GLOBAL_STATE_BYTES, Scalar, SnapshotDraft, SnapshotSummary,
    StateSet,
};

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/snapshots/record-with-global-state.json");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn draft(state: Option<GlobalState>) -> SnapshotDraft {
    SnapshotDraft {
        repo: "r".into(),
        name: "n".into(),
        uuid: "u".into(),
        storage_generation: None,
        shallow: false,
        start_millis: 1,
        end_millis: 2,
        global_state: state,
        indices: vec![],
    }
}

fn written(d: &SnapshotDraft) -> Vec<u8> {
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    let n = d.write(&mut buf).expect("fits");
    buf.truncate(n);
    buf
}

fn sample() -> GlobalState {
    let mut s = GlobalState::default();
    s.settings.push(("a".into(), Scalar::Str("1".into())));
    s.settings.push(("b".into(), Scalar::Bool(true)));
    s.settings.push(("c".into(), Scalar::Int(-3)));
    s.settings.push(("d".into(), Scalar::Number("1.5".into())));
    s.push(StateSet::IndexTemplates, "t".into(), br#"{"x":1}"#.to_vec());
    s.push(StateSet::StoredScripts, "sc\"q".into(), b"{}".to_vec());
    s
}

#[test]
fn the_record_the_java_wrote_is_read_in_full() {
    let state = GlobalState::find(&fixture()).expect("reads").expect("has");
    assert_eq!(state.settings.len(), 2);
    assert_eq!(state.settings[1], ("cluster.b".into(), Scalar::Bool(true)));
    for set in StateSet::ALL {
        assert_eq!(state.documents(set).len(), 1, "{}", set.key());
    }
    assert!(
        std::str::from_utf8(&state.documents(StateSet::StoredScripts)[0].1)
            .unwrap()
            .contains("painless")
    );
    // The summary readers skip the member.
    assert_eq!(
        SnapshotSummary::parse(&fixture()).expect("summary").name,
        "view"
    );
}

#[test]
fn what_is_written_is_read_back_and_an_empty_state_writes_nothing() {
    let record = written(&draft(Some(sample())));
    assert_eq!(GlobalState::find(&record).unwrap(), Some(sample()));
    SnapshotSummary::parse(&record).expect("summary skips it");
    for none in [None, Some(GlobalState::default())] {
        let bare = written(&draft(none));
        assert!(
            !String::from_utf8(bare.clone())
                .unwrap()
                .contains("global_state")
        );
        assert_eq!(GlobalState::find(&bare).unwrap(), None);
    }
}

#[test]
fn a_malformed_state_is_refused() {
    for bad in [
        r#"{"global_state":[]}"#,
        r#"{"global_state":{"persistent_settings":{"k":[1]}}}"#,
        r#"{"global_state":{"persistent_settings":{"k":null}}}"#,
        r#"{"global_state":{"persistent_settings":{"k":1,"k":2}}}"#,
        r#"{"global_state":{"persistent_settings":{},"persistent_settings":{}}}"#,
        r#"{"global_state":{"stored_scripts":{"s":1}}}"#,
        r#"{"global_state":{"stored_scripts":{"s":{},"s":{}}}}"#,
        r#"{"global_state":{"stored_scripts":[]}}"#,
    ] {
        assert!(
            matches!(
                GlobalState::find(bad.as_bytes()),
                Err(FormatError::Corrupt { .. })
            ),
            "{bad}"
        );
    }
}

#[test]
fn an_unknown_member_is_skipped() {
    let s = GlobalState::find(br#"{"global_state":{"future":{"a":1},"index_templates":{"t":{}}}}"#)
        .unwrap()
        .unwrap();
    assert_eq!(s.documents(StateSet::IndexTemplates).len(), 1);
}

fn with_script_of(len: usize) -> GlobalState {
    // `{"sc":` is 6 bytes inside `"stored_scripts":{...}` and the member wrapper.
    let mut s = GlobalState::default();
    let mut doc = br#"{"a":""#.to_vec();
    doc.resize(doc.len() + len, b'x');
    doc.extend_from_slice(br#""}"#);
    s.push(StateSet::StoredScripts, "sc".into(), doc);
    s
}

fn encoded_len(s: &GlobalState) -> usize {
    let r = written(&draft(Some(s.clone())));
    let bare = written(&draft(None));
    // `,"global_state":` is 16 bytes of framing the member's own length excludes.
    r.len() - bare.len() - ",\"global_state\":".len()
}

#[test]
fn a_state_of_exactly_the_limit_is_taken_and_one_byte_more_names_the_set() {
    let probe = encoded_len(&with_script_of(0));
    let at = with_script_of(MAX_GLOBAL_STATE_BYTES - probe);
    assert_eq!(encoded_len(&at), MAX_GLOBAL_STATE_BYTES);
    at.check_size().expect("at the limit");
    let over = with_script_of(MAX_GLOBAL_STATE_BYTES - probe + 1);
    match over.check_size() {
        Err(FormatError::TooLarge {
            what,
            limit,
            actual,
        }) => {
            assert_eq!(what, "stored_scripts");
            assert_eq!(limit, MAX_GLOBAL_STATE_BYTES as u64);
            assert_eq!(actual, MAX_GLOBAL_STATE_BYTES as u64 + 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn settings_that_pass_the_limit_name_the_settings() {
    let mut s = GlobalState::default();
    s.settings
        .push(("k".into(), Scalar::Str("x".repeat(MAX_GLOBAL_STATE_BYTES))));
    match s.check_size() {
        Err(FormatError::TooLarge { what, .. }) => assert_eq!(what, "persistent_settings"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_state_past_the_limit_is_refused_on_read() {
    let mut record = br#"{"global_state":{"stored_scripts":{"s":{"a":""#.to_vec();
    record.resize(MAX_GLOBAL_STATE_BYTES + 100, b'x');
    record.extend_from_slice(br#""}}}}"#);
    assert!(matches!(
        GlobalState::find(&record),
        Err(FormatError::TooLarge { .. })
    ));
}

fn record_with_state_of(len: usize) -> Vec<u8> {
    let head = br#"{"global_state":{"stored_scripts":{"s":{"a":""#;
    let tail = br#""}}}}"#;
    // The member's object is everything between `"global_state":` and the last `}`.
    // The record closes with the tail's last brace; the member does not.
    let fixed = head.len() + tail.len() - br#"{"global_state":"#.len() - 1;
    let mut record = head.to_vec();
    record.resize(head.len() + (len - fixed), b'x');
    record.extend_from_slice(tail);
    record
}

#[test]
fn a_state_of_exactly_the_limit_is_read_and_one_byte_more_is_refused() {
    let at = record_with_state_of(MAX_GLOBAL_STATE_BYTES);
    assert!(GlobalState::find(&at).unwrap().is_some());
    assert!(matches!(
        GlobalState::find(&record_with_state_of(MAX_GLOBAL_STATE_BYTES + 1)),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_record_of_exactly_the_limit_is_read_and_one_byte_more_is_refused() {
    let mut record = br#"{"name":"n"}"#.to_vec();
    record.resize(oss_format::MAX_SNAPSHOT_RECORD_BYTES, b' ');
    assert_eq!(GlobalState::find(&record).unwrap(), None);
    record.push(b' ');
    assert!(matches!(
        GlobalState::find(&record),
        Err(FormatError::Corrupt { .. })
    ));
}
