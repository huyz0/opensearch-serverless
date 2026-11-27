//! Small encoders the index builder shares.

use lucene_index::index_writer::InvertedTerm;

pub(super) fn term(bytes: &[u8]) -> InvertedTerm {
    InvertedTerm {
        term: bytes.to_vec(),
        freq: 1,
        positions: Vec::new(),
        offsets: Vec::new(),
        payloads: Vec::new(),
    }
}

/// `Uid.encodeId` of an id that is neither numeric nor URL-safe base64.
pub(super) fn encode_id(id: &str) -> Vec<u8> {
    let mut out = vec![0xff];
    out.extend_from_slice(id.as_bytes());
    out
}

/// `NumericUtils.longToSortableBytes`.
pub(super) fn sortable(v: i64) -> Vec<u8> {
    (v ^ i64::MIN).to_be_bytes().to_vec()
}

pub(super) fn register(value: &str) -> Vec<u8> {
    let mut out = 1u64.to_be_bytes().to_vec();
    out.extend_from_slice(value.as_bytes());
    out
}

pub(super) fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
