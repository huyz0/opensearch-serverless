//! Why the reader could not open or read a shard.

use std::fmt;

use oss_blob::BlobError;
use oss_format::FormatError;

/// A reader failure.
#[derive(Debug)]
pub enum ReaderError {
    /// The object store failed, or answered busy.
    Blob(BlobError),
    /// A blob did not decode as the shell's format.
    Format(FormatError),
    /// The engine could not open or read the commit.
    Engine(lucene_store::Error),
    /// The index is a tombstone: it was deleted.
    Deleted,
    /// The name is an alias, not an index.
    Alias,
    /// A restore is copying the index, or was interrupted before it finished.
    Restoring,
    /// The index sets `index.blocks.read`: it is not opened for a read.
    ReadBlocked(String),
    /// The engine could not open the commit's segments.
    Open(lucene_search::directory_reader::Error),
    /// A register blob is larger than [`crate::MAX_REGISTER_BYTES`].
    RegisterTooLarge,
    /// The manifest names a file without its length, which a reader needs
    /// to open it without a request per file.
    UnknownLength(String),
    /// The descriptor's mapping could not be read.
    Mapping(crate::mapping::MappingError),
    /// The point in time was taken over an earlier index of this name.
    NotHeld,
    /// The point in time has no finished freeze of the shard.
    NotFrozen(u32),
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReaderError::Blob(e) => write!(f, "object store: {e}"),
            ReaderError::Format(e) => write!(f, "blob format: {e}"),
            ReaderError::Engine(e) => write!(f, "engine: {e}"),
            ReaderError::Deleted => f.write_str("the index was deleted"),
            ReaderError::Alias => f.write_str("the name is an alias, not an index"),
            ReaderError::Restoring => f.write_str(oss_format::RESTORING_WHY),
            ReaderError::ReadBlocked(index) => {
                write!(f, "index [{index}] blocked by: [FORBIDDEN/7/index read (api)];")
            }
            ReaderError::Open(e) => write!(f, "engine: {e}"),
            ReaderError::RegisterTooLarge => f.write_str("register blob larger than its limit"),
            ReaderError::UnknownLength(name) => write!(f, "manifest gives no length for {name}"),
            ReaderError::Mapping(e) => write!(f, "{e}"),
            ReaderError::NotHeld => f.write_str(
                "the point in time was taken over an earlier index of this name that has since been deleted",
            ),
            ReaderError::NotFrozen(shard) => {
                write!(f, "the point in time did not freeze shard {shard}")
            }
        }
    }
}

impl std::error::Error for ReaderError {}

impl From<BlobError> for ReaderError {
    fn from(e: BlobError) -> Self {
        ReaderError::Blob(e)
    }
}

impl From<FormatError> for ReaderError {
    fn from(e: FormatError) -> Self {
        ReaderError::Format(e)
    }
}

impl From<lucene_search::directory_reader::Error> for ReaderError {
    fn from(e: lucene_search::directory_reader::Error) -> Self {
        ReaderError::Open(e)
    }
}

impl From<crate::mapping::MappingError> for ReaderError {
    fn from(e: crate::mapping::MappingError) -> Self {
        ReaderError::Mapping(e)
    }
}

impl From<lucene_store::Error> for ReaderError {
    fn from(e: lucene_store::Error) -> Self {
        ReaderError::Engine(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_names_its_cause() {
        let cases: [(ReaderError, &str); 7] = [
            (
                ReaderError::Mapping(crate::mapping::MappingError("bad".into())),
                "mapping: bad",
            ),
            (
                ReaderError::Blob(BlobError::Busy),
                "object store: no I/O buffer free",
            ),
            (ReaderError::Deleted, "the index was deleted"),
            (ReaderError::Alias, "the name is an alias, not an index"),
            (ReaderError::Restoring, oss_format::RESTORING_WHY),
            (
                ReaderError::RegisterTooLarge,
                "register blob larger than its limit",
            ),
            (
                ReaderError::UnknownLength("_0.si".into()),
                "manifest gives no length for _0.si",
            ),
        ];
        for (e, text) in cases {
            assert_eq!(e.to_string(), text);
        }
    }

    #[test]
    fn each_cause_converts_into_its_kind() {
        let blob: ReaderError = BlobError::NotFound.into();
        assert!(matches!(blob, ReaderError::Blob(BlobError::NotFound)));
        let format: ReaderError = FormatError::Truncated {
            what: "header",
            needed: 4,
            have: 1,
        }
        .into();
        assert!(format.to_string().starts_with("blob format: "), "{format}");
        let engine: ReaderError = lucene_store::Error::MalformedVarint.into();
        assert_eq!(
            engine.to_string(),
            "engine: malformed vint/vlong: too many bytes"
        );
        let mapping: ReaderError = crate::mapping::MappingError("bad".into()).into();
        assert!(matches!(mapping, ReaderError::Mapping(_)));
        let open: ReaderError =
            lucene_search::directory_reader::Error::Store(lucene_store::Error::MalformedVarint)
                .into();
        assert_eq!(
            open.to_string(),
            "engine: malformed vint/vlong: too many bytes"
        );
    }
}
