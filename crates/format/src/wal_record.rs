//! One write in a node's log: a document indexed or deleted, with the
//! sequence identity the engine gave it.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/nodelog/WalRecord.java`
//! (`toBytes`, `fromStream`, the accessors). A record is one compact JSON
//! object, written by OpenSearch's `XContentBuilder`:
//!
//! ```json
//! {"id":"1","source":"{\"a\":1}","deleted":true,"seq_no":7,"primary_term":2,"version":4,"routing":"r"}
//! ```
//!
//! `id` and `source` are always written; `deleted` only for a deletion;
//! `seq_no`, `primary_term` and `version` only when the record carries a
//! sequence identity (a sequence number other than -2 and a primary term
//! other than 0); `routing` only when there is one. The source is the
//! document's JSON as a string, escaped. A record never holds a raw newline,
//! which is what lets a node-log slice put one record a line.
//!
//! Read as leniently as the Java on unknown fields, which are skipped
//! whatever their type, and stricter on the known ones: a value of a type
//! the Java coerces (a number as text, text as a number or a flag, a
//! fraction, `null`) is refused, as are a repeated field and bytes after the
//! object (`fixtures/node-log/records-strict.txt`).

use std::borrow::Cow;

use crate::error::FormatError;
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "write-log record";

/// The sequence number of a record that has none: OpenSearch's
/// `SequenceNumbers.UNASSIGNED_SEQ_NO`.
pub const UNASSIGNED_SEQ_NO: i64 = -2;
/// The primary term of a record that has none:
/// `SequenceNumbers.UNASSIGNED_PRIMARY_TERM`.
pub const UNASSIGNED_PRIMARY_TERM: i64 = 0;
/// The version of a record that has none: `Versions.MATCH_ANY`.
pub const MATCH_ANY: i64 = -3;

/// One write, durable before it is acknowledged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalRecord<'a> {
    id: Cow<'a, str>,
    source: Cow<'a, str>,
    deletion: bool,
    seq_no: i64,
    primary_term: i64,
    version: i64,
    routing: Option<Cow<'a, str>>,
}

impl<'a> WalRecord<'a> {
    /// A document indexed, with the sequence identity the engine assigned.
    pub fn index(
        id: impl Into<Cow<'a, str>>,
        source: impl Into<Cow<'a, str>>,
        seq_no: i64,
        primary_term: i64,
        version: i64,
    ) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            deletion: false,
            seq_no,
            primary_term,
            version,
            routing: None,
        }
    }

    /// A document indexed with no sequence identity, replayed as a fresh
    /// operation.
    pub fn index_unsequenced(id: impl Into<Cow<'a, str>>, source: impl Into<Cow<'a, str>>) -> Self {
        Self::index(
            id,
            source,
            UNASSIGNED_SEQ_NO,
            UNASSIGNED_PRIMARY_TERM,
            MATCH_ANY,
        )
    }

    /// A document deleted, with the sequence identity the engine assigned.
    pub fn deletion(
        id: impl Into<Cow<'a, str>>,
        seq_no: i64,
        primary_term: i64,
        version: i64,
    ) -> Self {
        Self {
            deletion: true,
            ..Self::index(id, "", seq_no, primary_term, version)
        }
    }

    /// A document deleted with no sequence identity.
    pub fn deletion_unsequenced(id: impl Into<Cow<'a, str>>) -> Self {
        Self::deletion(id, UNASSIGNED_SEQ_NO, UNASSIGNED_PRIMARY_TERM, MATCH_ANY)
    }

    /// This record with a custom routing value, or none.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for an empty routing value, which the Java
    /// refuses.
    pub fn with_routing(
        mut self,
        routing: Option<impl Into<Cow<'a, str>>>,
    ) -> Result<Self, FormatError> {
        let routing = routing.map(Into::into);
        if routing.as_deref() == Some("") {
            return Err(crate::error::corrupt(WHAT, "a routing value is empty"));
        }
        self.routing = routing;
        Ok(self)
    }

    /// The document id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The document source, empty for a deletion.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether this record removes a document.
    pub fn is_deletion(&self) -> bool {
        self.deletion
    }

    /// The sequence number, or [`UNASSIGNED_SEQ_NO`].
    pub fn seq_no(&self) -> i64 {
        self.seq_no
    }

    /// The primary term, or [`UNASSIGNED_PRIMARY_TERM`].
    pub fn primary_term(&self) -> i64 {
        self.primary_term
    }

    /// The version, or [`MATCH_ANY`].
    pub fn version(&self) -> i64 {
        self.version
    }

    /// The custom routing value, `None` when the document is placed by id.
    pub fn routing(&self) -> Option<&str> {
        self.routing.as_deref()
    }

    /// The value the record is placed by: its routing, else its id.
    pub fn placed_by(&self) -> &str {
        self.routing.as_deref().unwrap_or(&self.id)
    }

    /// Whether the record replays as the operation it was, rather than as a
    /// fresh one.
    pub fn has_sequence_identity(&self) -> bool {
        self.seq_no != UNASSIGNED_SEQ_NO && self.primary_term != UNASSIGNED_PRIMARY_TERM
    }

    /// Writes the record as the Java's `toBytes` does into `buf`, returning
    /// its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `buf` is short: `actual` is the length
    /// needed.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.start_object();
        out.string_field("id", &self.id);
        out.string_field("source", &self.source);
        if self.deletion {
            out.bool_field("deleted", true);
        }
        if self.has_sequence_identity() {
            out.int_field("seq_no", self.seq_no);
            out.int_field("primary_term", self.primary_term);
            out.int_field("version", self.version);
        }
        if let Some(routing) = &self.routing {
            out.string_field("routing", routing);
        }
        out.end_object();
        out.finish(WHAT)
    }

    /// The record's length as [`WalRecord::write`] writes it.
    pub fn encoded_len(&self) -> usize {
        match self.write(&mut []) {
            Ok(n) => n,
            Err(FormatError::TooLarge { actual, .. }) => {
                usize::try_from(actual).unwrap_or(usize::MAX)
            }
            Err(_) => usize::MAX,
        }
    }

    /// The record written once, for a node-log blob to copy.
    ///
    /// # Errors
    ///
    /// None in practice; a write that fails its own measured length is
    /// [`FormatError::TooLarge`].
    pub fn encode(&self) -> Result<EncodedWalRecord, FormatError> {
        // ALLOC: the record's bytes, its id and source escaped: what the log holds for it until it lands.
        let mut bytes = vec![0u8; self.encoded_len()];
        let n = self.write(&mut bytes)?;
        bytes.truncate(n);
        Ok(EncodedWalRecord {
            bytes: bytes.into_boxed_slice(),
            seq_no: self.seq_no,
        })
    }

    /// Parses a record as the Java's `fromStream` does, borrowing every
    /// string that holds no escape.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, bytes after the object, a
    /// missing `id` or `source`, an empty `routing`, a repeated field, or a
    /// known field of a type the Java never writes there.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FormatError> {
        let mut json = Reader::new(bytes, WHAT);
        json.start_object("a record is an object")?;
        let mut seen = 0u8;
        let mut id = None;
        let mut source = None;
        let mut record = Self::index_unsequenced("", "");
        while let Some(field) = json.next_field()? {
            let Some(at) = FIELDS.iter().position(|f| *f == field) else {
                json.skip_value()?;
                continue;
            };
            let bit = 1u8 << at;
            if seen & bit != 0 {
                return Err(json.error("a record field is repeated"));
            }
            seen |= bit;
            match at {
                0 => id = Some(text(&mut json)?),
                1 => source = Some(text(&mut json)?),
                2 => {
                    let Token::Bool(deleted) = json.expect_token()? else {
                        return Err(json.error("deleted is a boolean"));
                    };
                    record.deletion = deleted;
                }
                3 => record.seq_no = int(&mut json)?,
                4 => record.primary_term = int(&mut json)?,
                5 => record.version = int(&mut json)?,
                _ => {
                    let routing = text(&mut json)?;
                    if routing.is_empty() {
                        return Err(json.error("a routing value is empty"));
                    }
                    record.routing = Some(routing);
                }
            }
        }
        json.finish()?;
        let (Some(id), Some(source)) = (id, source) else {
            return Err(json.error("a record needs id and source"));
        };
        record.id = id;
        record.source = source;
        Ok(record)
    }

    /// The record with every string owned, independent of the bytes it was
    /// parsed from.
    pub fn into_owned(self) -> WalRecord<'static> {
        WalRecord {
            id: Cow::Owned(self.id.into_owned()),
            source: Cow::Owned(self.source.into_owned()),
            deletion: self.deletion,
            seq_no: self.seq_no,
            primary_term: self.primary_term,
            version: self.version,
            routing: self.routing.map(|r| Cow::Owned(r.into_owned())),
        }
    }
}

const FIELDS: [&str; 7] = [
    "id",
    "source",
    "deleted",
    "seq_no",
    "primary_term",
    "version",
    "routing",
];

fn text<'a>(json: &mut Reader<'a>) -> Result<Cow<'a, str>, FormatError> {
    match json.expect_token()? {
        Token::Str(s) => Ok(s),
        _ => Err(json.error("a record's text field is a string")),
    }
}

fn int(json: &mut Reader<'_>) -> Result<i64, FormatError> {
    match json.expect_token()? {
        Token::Int(n) => Ok(n),
        _ => Err(json.error("a record's number is an integer")),
    }
}

/// A record's bytes as [`WalRecord::write`] wrote them, with the sequence
/// number a node-log slice's header reports. Made only by
/// [`WalRecord::encode`], so it never holds a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedWalRecord {
    bytes: Box<[u8]>,
    seq_no: i64,
}

impl EncodedWalRecord {
    /// The record's bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The record's sequence number, or [`UNASSIGNED_SEQ_NO`].
    pub fn seq_no(&self) -> i64 {
        self.seq_no
    }

    /// The record parsed back.
    ///
    /// # Errors
    ///
    /// None for bytes this type made; as [`WalRecord::parse`].
    pub fn record(&self) -> Result<WalRecord<'_>, FormatError> {
        WalRecord::parse(&self.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(record: &WalRecord<'_>) -> String {
        String::from_utf8(record.encode().unwrap().bytes().to_vec()).unwrap()
    }

    #[test]
    fn a_built_record_writes_only_the_fields_it_has() {
        assert_eq!(
            written(&WalRecord::index_unsequenced("1", "{}")),
            r#"{"id":"1","source":"{}"}"#
        );
        assert_eq!(
            written(&WalRecord::deletion("d", 7, 2, 4)),
            r#"{"id":"d","source":"","deleted":true,"seq_no":7,"primary_term":2,"version":4}"#
        );
        let routed = WalRecord::index("1", "x\ny", 0, 1, 1)
            .with_routing(Some("r"))
            .unwrap();
        assert_eq!(
            written(&routed),
            r#"{"id":"1","source":"x\ny","seq_no":0,"primary_term":1,"version":1,"routing":"r"}"#
        );
        assert_eq!(routed.placed_by(), "r");
        assert_eq!(WalRecord::deletion_unsequenced("z").placed_by(), "z");
    }

    #[test]
    fn identity_needs_both_a_sequence_number_and_a_term() {
        assert!(WalRecord::index("1", "", 0, 1, 1).has_sequence_identity());
        assert!(!WalRecord::index("1", "", UNASSIGNED_SEQ_NO, 1, 1).has_sequence_identity());
        assert!(!WalRecord::index("1", "", 5, UNASSIGNED_PRIMARY_TERM, 1).has_sequence_identity());
        assert!(WalRecord::index("1", "", -1, -5, MATCH_ANY).has_sequence_identity());
    }

    #[test]
    fn an_empty_routing_is_refused_and_none_clears_it() {
        assert!(
            WalRecord::index_unsequenced("1", "")
                .with_routing(Some(""))
                .is_err()
        );
        let cleared = WalRecord::index_unsequenced("1", "")
            .with_routing(Some("r"))
            .unwrap()
            .with_routing(None::<&str>)
            .unwrap();
        assert_eq!(cleared.routing(), None);
    }

    #[test]
    fn every_repeated_field_is_refused_not_only_the_first() {
        for field in FIELDS {
            let value = match field {
                "deleted" => "true",
                "seq_no" | "primary_term" | "version" => "1",
                _ => "\"x\"",
            };
            let json = format!(r#"{{"id":"1","source":"","{field}":{value},"{field}":{value}}}"#);
            assert!(WalRecord::parse(json.as_bytes()).is_err(), "{json}");
        }
    }

    #[test]
    fn a_short_buffer_reports_the_length_needed() {
        let record = WalRecord::index_unsequenced("1", "{}");
        let mut buf = [0u8; 4];
        assert_eq!(
            record.write(&mut buf),
            Err(FormatError::TooLarge {
                what: WHAT,
                limit: 4,
                actual: 24
            })
        );
        assert_eq!(record.encoded_len(), 24);
    }

    #[test]
    fn plain_strings_are_borrowed_and_into_owned_keeps_the_record() {
        let bytes = br#"{"id":"1","source":"plain","routing":"r"}"#;
        let record = WalRecord::parse(bytes).unwrap();
        assert!(matches!(record.id, Cow::Borrowed(_)));
        assert!(matches!(record.source, Cow::Borrowed(_)));
        let owned = record.clone().into_owned();
        assert_eq!(owned, record);
        let encoded = owned.encode().unwrap();
        assert_eq!(encoded.record().unwrap(), record);
        assert_eq!(encoded.seq_no(), UNASSIGNED_SEQ_NO);
    }
}
