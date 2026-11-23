//! What a snapshot record names of the indexes it captured: the pins the
//! orphan-shard sweep honours.
//!
//! Read from the Java's `SnapshotRecord.toBytes`/`fromStream` format: an
//! object whose `indices` is an object from the index's name to an object
//! with its `uuid` (and, once captured, `shards`). The record is read for
//! the index and uuid of every entry and nothing else: an entry pins its
//! whole index incarnation, whether the capture is in progress (no
//! `shards`), finished, shallow or standard, which holds at least what the
//! Java's `referencedBlobs` and `capturing` would.

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};

const WHAT: &str = "snapshot record";

/// The longest record a reader takes; a longer one was not written by a
/// store and reads as unreadable.
pub const MAX_SNAPSHOT_RECORD_BYTES: usize = 16 << 20;

/// The `(index name, uuid)` of every index `value` captured.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for malformed JSON, an `indices` that is not an
/// object, an entry that is not an object or has no nonempty `uuid`, or a
/// record past [`MAX_SNAPSHOT_RECORD_BYTES`].
pub fn snapshot_pinned_indexes(value: &[u8]) -> Result<Vec<(String, String)>, FormatError> {
    if value.len() > MAX_SNAPSHOT_RECORD_BYTES {
        return Err(corrupt(WHAT, "a record past the size limit"));
    }
    let mut json = Reader::new(value, WHAT);
    json.start_object("a snapshot record is an object")?;
    // ALLOC: one entry an index of the snapshot, bounded by the record size.
    let mut pinned: Vec<(String, String)> = Vec::new();
    let mut seen = false;
    while let Some(field) = json.next_field()? {
        if field.as_ref() != "indices" {
            json.skip_value()?;
            continue;
        }
        if seen {
            return Err(json.error("a snapshot record's indices are repeated"));
        }
        seen = true;
        if json.expect_token()? != Token::StartObject {
            return Err(json.error("a snapshot's indices are an object"));
        }
        while let Some(name) = json.next_field()? {
            if json.expect_token()? != Token::StartObject {
                return Err(json.error("a snapshot's index entry is an object"));
            }
            let mut uuid: Option<String> = None;
            while let Some(key) = json.next_field()? {
                if key.as_ref() == "uuid" {
                    match json.expect_token()? {
                        Token::Str(s) if !s.is_empty() => uuid = Some(s.into_owned()),
                        _ => return Err(json.error("an index uuid is a nonempty string")),
                    }
                } else {
                    json.skip_value()?;
                }
            }
            let uuid = uuid.ok_or_else(|| json.error("a snapshot's index entry has no uuid"))?;
            // ALLOC: one name, as above.
            pinned.push((name.into_owned(), uuid));
        }
    }
    json.finish()?;
    if !seen {
        return Err(corrupt(WHAT, "a snapshot record names its indices"));
    }
    Ok(pinned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_captured_index_is_named_with_its_uuid() {
        let record = br#"{"repo":"r","name":"s","uuid":"u","shallow":true,
            "start_time_in_millis":1,"end_time_in_millis":2,
            "indices":{"logs":{"uuid":"U1","number_of_shards":2,"mapping":"{}",
              "settings":{"index":{"a":[1,2]}},
              "shards":[{"shard":0,"term":1,"files":{"f":"loc"}}]},
              "other":{"uuid":"U2","number_of_shards":1}}}"#;
        assert_eq!(
            snapshot_pinned_indexes(record).expect("reads"),
            [
                ("logs".to_owned(), "U1".to_owned()),
                ("other".to_owned(), "U2".to_owned())
            ]
        );
    }

    #[test]
    fn a_record_that_cannot_be_read_is_an_error_not_no_pins() {
        for bad in [
            &br#"{"name":"s"}"#[..],
            br#"{"indices":[]}"#,
            br#"{"indices":{"a":{"number_of_shards":1}}}"#,
            br#"{"indices":{"a":{"uuid":""}}}"#,
            br#"{"indices":{"a":{"uuid":"u"}},"indices":{}}"#,
            br#"{"indices":{"a":{"uuid":"u"}"#,
            b"",
        ] {
            assert!(snapshot_pinned_indexes(bad).is_err(), "{bad:?}");
        }
    }
}
