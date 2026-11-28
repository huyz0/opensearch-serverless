//! The bytes of the `state.*` operations (docs/specs/plugin-system, Host API 1.0).

use oss_ext::rest::CodecError;
use oss_ext::state::{
    MAX_KEY_BYTES, MAX_VALUE_BYTES, Put, decode_found, decode_key, decode_put, encode_found,
    encode_key, encode_put,
};

#[test]
fn a_key_and_a_put_round_trip() {
    assert_eq!(decode_key(&encode_key("cursor")).unwrap(), "cursor");
    for if_absent in [false, true] {
        let p = Put {
            key: "k".to_owned(),
            value: b"v".to_vec(),
            if_absent,
        };
        assert_eq!(decode_put(&encode_put(&p)).unwrap(), p);
    }
    let empty = Put {
        key: "k".to_owned(),
        value: Vec::new(),
        if_absent: false,
    };
    assert_eq!(decode_put(&encode_put(&empty)).unwrap(), empty);
}

#[test]
fn an_answer_says_missing_apart_from_empty() {
    assert_eq!(decode_found(&encode_found(None)).unwrap(), None);
    assert_eq!(
        decode_found(&encode_found(Some(b""))).unwrap(),
        Some(Vec::new())
    );
    assert_eq!(
        decode_found(&encode_found(Some(b"x"))).unwrap(),
        Some(b"x".to_vec())
    );
    assert_eq!(decode_found(&[]), Err(CodecError::Short));
    assert_eq!(decode_found(&[0, 1]), Err(CodecError::Value));
    assert_eq!(decode_found(&[2]), Err(CodecError::Value));
}

#[test]
fn a_key_and_a_value_are_bounded_before_they_are_read() {
    let longest = "k".repeat(MAX_KEY_BYTES);
    assert!(decode_key(&encode_key(&longest)).is_ok());
    assert_eq!(
        decode_key(&encode_key(&format!("{longest}k"))),
        Err(CodecError::Bound)
    );
    assert_eq!(decode_key(&encode_key("")), Err(CodecError::Bound));
    let mut p = Put {
        key: "k".to_owned(),
        value: vec![0; MAX_VALUE_BYTES],
        if_absent: false,
    };
    assert!(decode_put(&encode_put(&p)).is_ok());
    p.value.push(0);
    assert_eq!(decode_put(&encode_put(&p)), Err(CodecError::Bound));
}

#[test]
fn bytes_that_are_not_exactly_a_message_are_refused() {
    let mut k = encode_key("a");
    k.push(0);
    assert_eq!(decode_key(&k), Err(CodecError::Trailing));
    assert_eq!(decode_key(&[2, 1, 0, b'a']), Err(CodecError::Version));
    assert_eq!(decode_key(&[1, 5, 0, b'a']), Err(CodecError::Short));
    assert_eq!(decode_key(&[1, 1, 0, 0xff]), Err(CodecError::Text));
    let mut p = encode_put(&Put {
        key: "a".to_owned(),
        value: b"v".to_vec(),
        if_absent: false,
    });
    p.push(0);
    assert_eq!(decode_put(&p), Err(CodecError::Trailing));
    let mut bad_flag = encode_put(&Put {
        key: "a".to_owned(),
        value: Vec::new(),
        if_absent: false,
    });
    bad_flag[1] = 2;
    assert_eq!(decode_put(&bad_flag), Err(CodecError::Value));
    assert_eq!(decode_put(&[1]), Err(CodecError::Short));
}
