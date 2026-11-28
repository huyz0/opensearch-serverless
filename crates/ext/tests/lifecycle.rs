//! The lifecycle point's codec (docs/specs/plugin-system AC-5, AC-17).

use oss_ext::lifecycle::{Event, MAX_PART_BYTES};
use oss_ext::rest::CodecError;

fn event() -> Event {
    Event {
        kind: "index.created".into(),
        tenant: "t1".into(),
        index: "logs".into(),
        at_ms: 0x0102,
    }
}

#[test]
fn golden_bytes() {
    let mut want = vec![1, 13, 0];
    want.extend_from_slice(b"index.created");
    want.extend_from_slice(&[2, 0]);
    want.extend_from_slice(b"t1");
    want.extend_from_slice(&[4, 0]);
    want.extend_from_slice(b"logs");
    want.extend_from_slice(&[2, 1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(event().encode(), want);
    assert_eq!(Event::decode(&want), Ok(event()));
}

#[test]
fn malformed_bytes_are_refused() {
    let good = event().encode();
    let mut bad_version = good.clone();
    bad_version[0] = 2;
    assert_eq!(Event::decode(&bad_version), Err(CodecError::Version));
    assert_eq!(Event::decode(&[]), Err(CodecError::Short));
    assert_eq!(
        Event::decode(&good[..good.len() - 1]),
        Err(CodecError::Short)
    );
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(Event::decode(&trailing), Err(CodecError::Trailing));
    let mut text = good;
    text[3] = 0xff;
    assert_eq!(Event::decode(&text), Err(CodecError::Text));
}

#[test]
fn a_part_is_bounded_inclusive() {
    let with = |n: usize| Event {
        kind: "k".repeat(n),
        ..event()
    };
    let ok = with(MAX_PART_BYTES).encode();
    assert!(Event::decode(&ok).is_ok(), "exactly the ceiling");
    let mut over = vec![1];
    over.extend_from_slice(
        &u16::try_from(MAX_PART_BYTES + 1)
            .expect("fits")
            .to_le_bytes(),
    );
    over.extend_from_slice(&vec![b'k'; MAX_PART_BYTES + 1]);
    assert_eq!(Event::decode(&over), Err(CodecError::Bound));
}
