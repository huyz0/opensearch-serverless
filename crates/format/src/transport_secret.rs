//! The transport secret register's value: the keys every node signs and
//! verifies transport requests with.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/TransportSecrets.java`
//! (`parse`, `toJson`). The value is a JSON object, `generation`, `current`,
//! `previous` and `rotated_at`, or a bare secret from before rotation
//! existed, read as generation 1 with no previous key. The register's own
//! generation replaces the one written, so a reader takes it from the read.
//!
//! An error never carries the value's bytes: they are the secret.
//! Like the Java's strict parser this refuses a repeated field; a number of
//! any kind reads as the Java's `longValue` reads it, a fraction rounded
//! toward zero; unknown fields are skipped; blank means Java's
//! `Character.isWhitespace` throughout.

use crate::error::FormatError;
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "transport secret";

/// The longest value read or written: two 64-digit keys and the numbers
/// with room to spare.
pub const MAX_TRANSPORT_SECRET_BYTES: usize = 1024;

/// One read of the register (`TransportSecrets`).
#[derive(Clone, PartialEq, Eq)]
pub struct TransportSecret {
    /// The generation written; a reader replaces it with the register's.
    pub generation: i64,
    /// The current key.
    pub current: String,
    /// The key it replaced, accepted for one lease TTL after `rotated_at`.
    pub previous: Option<String>,
    /// When `current` replaced `previous`, in epoch milliseconds.
    pub rotated_at_millis: i64,
}

impl std::fmt::Debug for TransportSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the keys.
        f.debug_struct("TransportSecret")
            .field("generation", &self.generation)
            .field("previous", &self.previous.is_some())
            .field("rotated_at_millis", &self.rotated_at_millis)
            .finish_non_exhaustive()
    }
}

impl TransportSecret {
    /// Parses the register's value as the Java's `parse`: surrounding
    /// whitespace dropped, a value not starting with `{` a bare current key
    /// at generation 1, otherwise one object whose `current` is a string
    /// that is not blank, whose `previous` is absent, null or such a string,
    /// and whose numbers default to 1 and 0.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a blank value, bytes that are not UTF-8
    /// or one JSON object, a repeated field, or a missing or blank key.
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        let corrupt = |reason| FormatError::Corrupt { what: WHAT, reason };
        let text = std::str::from_utf8(value).map_err(|_| corrupt("not UTF-8"))?;
        // `String.trim`: every char at or below a space.
        let trimmed = text.trim_matches(|c: char| c <= ' ');
        if trimmed.is_empty() {
            return Err(corrupt("blank secret"));
        }
        if !trimmed.starts_with('{') {
            return Ok(Self {
                generation: 1,
                // ALLOC: one key, the register's own bytes.
                current: trimmed.to_owned(),
                previous: None,
                rotated_at_millis: 0,
            });
        }
        let mut json = Reader::new(trimmed.as_bytes(), WHAT);
        json.start_object("expected an object")?;
        let (mut generation, mut current, mut previous, mut rotated_at) = (None, None, None, None);
        while let Some(field) = json.next_field()? {
            match field.as_ref() {
                "generation" => set(&mut generation, number(&mut json)?)?,
                "rotated_at" => set(&mut rotated_at, number(&mut json)?)?,
                "current" => set(&mut current, key(&mut json)?)?,
                "previous" => set(&mut previous, key(&mut json)?)?,
                _ => json.skip_value()?,
            }
        }
        json.finish()?;
        let blank = |k: &str| k.chars().all(java_whitespace);
        let current = current
            .flatten()
            .filter(|k| !blank(k))
            .ok_or(corrupt("current must be a nonblank string"))?;
        let previous = previous.flatten();
        if previous.as_deref().is_some_and(blank) {
            return Err(corrupt("previous must be a nonblank string"));
        }
        Ok(Self {
            generation: generation.flatten().unwrap_or(1),
            current,
            previous,
            rotated_at_millis: rotated_at.flatten().unwrap_or(0),
        })
    }

    /// Writes the value as the Java's `toJson`: the four fields in order,
    /// `previous` as null when there is none, into the front of `buf`, and
    /// returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `buf` is shorter than the value.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let mut out = Out::new(buf);
        out.start_object();
        out.int_field("generation", self.generation);
        out.string_field("current", &self.current);
        match &self.previous {
            Some(previous) => out.string_field("previous", previous),
            None => out.null_field("previous"),
        }
        out.int_field("rotated_at", self.rotated_at_millis);
        out.end_object();
        out.finish(WHAT)
    }
}

fn set<T>(slot: &mut Option<T>, value: T) -> Result<(), FormatError> {
    if slot.replace(value).is_some() {
        return Err(FormatError::Corrupt {
            what: WHAT,
            reason: "a field is repeated",
        });
    }
    Ok(())
}

/// A number as the Java's `Number.longValue`, or `None` for anything else,
/// which the Java reads as the default.
fn number(json: &mut Reader<'_>) -> Result<Option<i64>, FormatError> {
    Ok(match json.expect_token()? {
        Token::Int(n) => Some(n),
        Token::OtherNumber => json
            .number_text()
            .parse::<f64>()
            .ok()
            // `as` saturates and drops the fraction, as `double.longValue`.
            .map(|n| n as i64),
        first => {
            json.skip_after(&first)?;
            None
        }
    })
}

/// A key: a string, or `None` for null.
fn key(json: &mut Reader<'_>) -> Result<Option<String>, FormatError> {
    match json.expect_token()? {
        Token::Str(s) => Ok(Some(s.into_owned())),
        Token::Null => Ok(None),
        _ => Err(json.error("a key must be a string")),
    }
}

/// Java's `Character.isWhitespace`, which `String.isBlank` uses: the space,
/// line and paragraph separators but the no-break ones, and the ASCII
/// controls tab to carriage return and file to unit separator.
fn java_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{1680}'
            | '\u{2000}'..='\u{2006}'
            | '\u{2008}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{205f}'
            | '\u{3000}'
    )
}
