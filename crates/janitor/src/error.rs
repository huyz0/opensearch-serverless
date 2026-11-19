//! Why a sweep did not finish.

use std::fmt;

use oss_blob::BlobError;
use oss_format::FormatError;

/// A sweep failure. Nothing is deleted on one that precedes the delete, and
/// a failed delete leaves the sweep's memory as it was.
#[derive(Debug)]
pub enum JanitorError {
    /// The store failed.
    Blob(BlobError),
    /// The manifest or a key was not what the format allows.
    Format(FormatError),
    /// The pins could not be read: unreadable is not "none".
    Pins(&'static str),
}

impl fmt::Display for JanitorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob(e) => e.fmt(f),
            Self::Format(e) => e.fmt(f),
            Self::Pins(why) => write!(f, "the pins could not be read: {why}"),
        }
    }
}

impl std::error::Error for JanitorError {}

impl From<BlobError> for JanitorError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for JanitorError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

#[cfg(test)]
mod tests {
    use super::JanitorError;

    #[test]
    fn an_unreadable_pin_says_so() {
        let e = JanitorError::Pins("a view record");
        assert_eq!(e.to_string(), "the pins could not be read: a view record");
    }
}
