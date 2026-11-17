//! The search DSL subset: parsing a `_search` body into what to run.
//!
//! A body is fed chunk by chunk to `oss_rest`'s streaming tokenizer, whose
//! tokens a [`Recorder`] keeps within a byte limit; [`SearchSource::parse`]
//! then reads them with the ported OpenSearch parsers. No JSON tree is
//! built and the body is never held whole. The subset is `match_all`,
//! `term`, `match`, `range` and `bool`, the queries of [`queries`], with
//! `size`, `from` and `sort`.

pub mod bool_query;
pub mod error;
pub mod queries;
pub mod query;
pub mod registered;
pub mod sort;
pub mod source;
pub mod tokens;
pub mod values;

pub use bool_query::BoolQuery;
pub use error::DslError;
pub use query::{Operator, Query};
pub use sort::SortKey;
pub use source::SearchSource;
pub use tokens::Recorder;
pub use values::Value;

/// The most bytes a search body's recorded tokens may take.
pub const MAX_RECORDED_BYTES: usize = 1 << 20;

/// Parses a whole body given as chunks: for tests and callers that already
/// hold the bytes in pieces.
pub fn parse_chunks<'a>(
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> Result<SearchSource, DslError> {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokenizer = source::tokenizer();
    for chunk in chunks {
        tokenizer.feed(chunk, &mut rec)?;
    }
    tokenizer.finish(&mut rec)?;
    SearchSource::parse(&rec)
}
