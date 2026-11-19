//! The index tables of `_list/indices` and `_cat/indices`: the row, what a
//! request asks for, and the token that resumes a prefix walk.
//!
//! `_list` answers a page at a time; `_cat` answers the whole of what its
//! pattern resolves to under the pattern cap and is never paged. The row has
//! no document count or size: those are shard facts and nothing here reads
//! shards, so they are left out rather than reported as zero.

use super::table::Table;
use super::token;
use crate::route::resolve::{Refusal, is_prefix_pattern};

/// Rows in a page when `size` is not given.
pub const DEFAULT_SIZE: usize = 500;

/// The most rows a page may ask for.
pub const MAX_SIZE: usize = 5000;

/// The version a token starts with.
const TOKEN_VERSION: &str = "1\n";

/// What a `_list/indices` request asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paging {
    /// The prefix walked; nothing for named indices, answered in one page.
    pub prefix: Option<String>,
    /// The name to resume after; nothing for the first page.
    pub after: Option<String>,
    /// Rows in a page.
    pub size: usize,
}

/// The empty index table.
#[must_use]
pub fn table() -> Table<6> {
    Table::new(["index", "uuid", "pri", "rep", "status", "mapping_version"])
}

/// Adds an index's row: always `open` with no replicas, as there is no closed
/// state and no replica to report.
pub fn row(table: &mut Table<6>, name: &str, uuid: &str, shards: u32, mapping_version: u64) {
    // ALLOC: the six cells of one row, bounded by the name and uuid.
    table.row([
        name.to_owned(),
        uuid.to_owned(),
        // ALLOC: the numeric cells of one row.
        shards.to_string(),
        // ALLOC: constant cells.
        "0".to_owned(),
        "open".to_owned(),
        // ALLOC: the version cell.
        mapping_version.to_string(),
    ]);
}

/// The token that resumes a walk over `prefix` after `after`.
#[must_use]
pub fn token(prefix: &str, after: &str) -> String {
    // ALLOC: one token, bounded by a name.
    token::encode(format!("{TOKEN_VERSION}{prefix}\n{after}").as_bytes())
}

fn illegal(reason: impl Into<String>) -> Refusal {
    Refusal::new(400, "illegal_argument_exception", reason)
}

fn tainted() -> Refusal {
    illegal(
        "Parameter [next_token] has been tainted and is incorrect. Please provide a valid \
         [next_token].",
    )
}

/// Decides a request over `index`: the refusal, or the walk it asks for
/// (nothing for a `_cat` request, which is not paged).
///
/// # Errors
///
/// A [`Refusal`] in the shell's words: an unsupported cat column option, a
/// `_cat` request that pages, a `size` that is not a number or outside 1 to
/// [`MAX_SIZE`], a `next_token` on anything but one prefix pattern, or a
/// token that does not decode or belongs to another prefix.
pub fn plan(
    listing: bool,
    index: &str,
    size: Option<&str>,
    next_token: Option<&str>,
    cat_unsupported: Option<&str>,
) -> Result<Option<Paging>, Refusal> {
    if let Some(reason) = cat_unsupported {
        return Err(Refusal::new(501, "unsupported_cat_parameter", reason));
    }
    if !listing {
        if size.is_some() || next_token.is_some() {
            return Err(illegal(
                "_cat/indices is not paginated; _list/indices takes size and next_token",
            ));
        }
        return Ok(None);
    }
    let size = page_size(size)?;
    // Java's `String.trim`: characters up to a space, not all Unicode space.
    let expression = index.trim_matches(|c: char| c <= ' ');
    if expression.contains(',') || !is_prefix_pattern(expression) {
        if next_token.is_some() {
            // ALLOC: one message, bounded by the expression.
            return Err(illegal(format!(
                "next_token resumes a walk over one prefix pattern, and [{index}] is not one"
            )));
        }
        return Ok(Some(Paging {
            prefix: None,
            after: None,
            size,
        }));
    }
    let prefix = expression.strip_suffix('*').unwrap_or(expression);
    let after = match next_token {
        None => None,
        Some(text) => {
            let bytes = token::decode(text).ok_or_else(tainted)?;
            let decoded = String::from_utf8_lossy(&bytes);
            let rest = decoded.strip_prefix(TOKEN_VERSION).ok_or_else(tainted)?;
            let (token_prefix, after) = rest.split_once('\n').ok_or_else(tainted)?;
            if token_prefix != prefix {
                return Err(tainted());
            }
            // ALLOC: one name.
            Some(after.to_owned())
        }
    };
    Ok(Some(Paging {
        // ALLOC: one prefix.
        prefix: Some(prefix.to_owned()),
        after,
        size,
    }))
}

fn page_size(size: Option<&str>) -> Result<usize, Refusal> {
    let Some(text) = size else {
        return Ok(DEFAULT_SIZE);
    };
    // ALLOC: one message, bounded by the text.
    let n = text
        .parse::<i32>()
        .map_err(|_| illegal(format!("size must be a number, got [{text}]")))?;
    match usize::try_from(n) {
        Ok(n) if (1..=MAX_SIZE).contains(&n) => Ok(n),
        // ALLOC: one message.
        _ => Err(illegal(format!(
            "size must be between 1 and [{MAX_SIZE}], got [{n}]"
        ))),
    }
}
