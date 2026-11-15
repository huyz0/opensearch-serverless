//! The ways an object-store request can fail.
//!
//! The kinds follow what S3 reports, so callers handle the emulator and the
//! real client alike. The Java shell surfaces these as `IOException`
//! subclasses (`NoSuchFileException`, `FileAlreadyExistsException`); here
//! they are one enum the caller can match on.

use std::fmt;

/// Why a store request failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobError {
    /// The key does not exist (S3 404 `NoSuchKey`).
    NotFound,
    /// The requested range starts at or past the object's end (S3 416).
    InvalidRange,
    /// The store asked the client to slow down (S3 503 `SlowDown`).
    Throttled,
    /// Another conditional write to the key is in flight (S3 409
    /// `ConditionalRequestConflict`). Unlike a 412 it does not mean the key
    /// is taken: the other write may still fail, so retry.
    Conflict,
    /// The store failed the request; retrying may succeed (S3 5xx, a
    /// dropped connection).
    Unavailable,
    /// The emulator's configured capacity would be exceeded. Real S3 never
    /// returns this.
    CapacityExceeded,
    /// The caller passed something the store cannot accept: an empty
    /// buffer, an invalid key, a page size of zero, or a body whose length
    /// differs from the one it declared.
    InvalidArgument(&'static str),
    /// The caller's source or sink failed while the body streamed.
    Stream(&'static str),
    /// No I/O buffer was free in the node's buffer pool; nothing was sent.
    /// The caller answers 429 or backs off (ADR 0004).
    Busy,
}

impl fmt::Display for BlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobError::NotFound => f.write_str("no such key"),
            BlobError::InvalidRange => f.write_str("range starts at or past the end of the object"),
            BlobError::Throttled => f.write_str("throttled by the store"),
            BlobError::Conflict => f.write_str("conflicting conditional write in flight"),
            BlobError::Unavailable => f.write_str("store unavailable"),
            BlobError::CapacityExceeded => f.write_str("emulator capacity exceeded"),
            BlobError::InvalidArgument(why) => write!(f, "invalid argument: {why}"),
            BlobError::Stream(why) => write!(f, "body stream failed: {why}"),
            BlobError::Busy => f.write_str("no I/O buffer free"),
        }
    }
}

impl std::error::Error for BlobError {}

/// The result of a store request.
pub type BlobResult<T> = Result<T, BlobError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_displays_its_reason() {
        let cases = [
            (BlobError::NotFound, "no such key"),
            (
                BlobError::InvalidRange,
                "range starts at or past the end of the object",
            ),
            (BlobError::Throttled, "throttled by the store"),
            (
                BlobError::Conflict,
                "conflicting conditional write in flight",
            ),
            (BlobError::Unavailable, "store unavailable"),
            (BlobError::CapacityExceeded, "emulator capacity exceeded"),
            (
                BlobError::InvalidArgument("empty key"),
                "invalid argument: empty key",
            ),
            (
                BlobError::Stream("sink closed"),
                "body stream failed: sink closed",
            ),
            (BlobError::Busy, "no I/O buffer free"),
        ];
        for (err, text) in cases {
            assert_eq!(err.to_string(), text);
        }
    }
}
