//! `inner_hits` of a `nested` query: `InnerHitBuilder.fromXContent` at
//! OpenSearch 3.10.0, for the keys this node serves (`name`, `from`, `size`,
//! `sort` and `_source` as a boolean).

use crate::dsl::error::DslError;
use crate::dsl::sort::{SortKey, parse_sort};
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::int_value;
use crate::fetch::source_filter::parse_source;

/// `InnerHitBuilder.DEFAULT_SIZE`.
pub const DEFAULT_SIZE: usize = 3;

/// What a nested query's `inner_hits` asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct InnerHits {
    /// The name the hits are reported under; the path when absent.
    pub name: Option<String>,
    /// How many children to skip.
    pub from: usize,
    /// How many children to return.
    pub size: usize,
    /// The sort of the children; by score when empty.
    pub sort: Vec<SortKey>,
    /// `_source: false`: no `_source` in the children.
    pub source_skip: bool,
}

/// A key `InnerHitBuilder` reads that this node does not serve yet.
fn unsupported(key: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::unsupported(&format!("[inner_hits] [{key}]"))
}

fn unknown(p: &Parser<'_>, key: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::parsing(p.location(), format!("[inner_hits] unknown field [{key}]"))
}

/// `from` or `size`: `InnerHitBuilder.setFrom` and `setSize`.
fn window(n: i32, word: &str) -> Result<usize, DslError> {
    // ALLOC: error path; one message.
    usize::try_from(n)
        .map_err(|_| DslError::illegal(format!("illegal {word} value, at least 0 or higher")))
}

/// Reads the options object; the parser is on its start.
pub fn parse_inner_hits(p: &mut Parser<'_>) -> Result<InnerHits, DslError> {
    let mut hits = InnerHits {
        name: None,
        from: 0,
        size: DEFAULT_SIZE,
        // ALLOC: empty.
        sort: Vec::new(),
        source_skip: false,
    };
    let mut key = "";
    while let Some(token) = p.next_token() {
        if token == Kind::EndObject {
            break;
        }
        if token == Kind::FieldName {
            key = p.text();
            continue;
        }
        match key {
            "name" if token == Kind::String => {
                // ALLOC: one name a nested query.
                hits.name = Some(p.text().to_owned());
            }
            "from" if token.is_value() => hits.from = window(int_value(p)?, "from")?,
            "size" if token.is_value() => hits.size = window(int_value(p)?, "size")?,
            "sort" => hits.sort = parse_sort(p)?,
            "_source" => {
                let filter = parse_source(p, key)?;
                if !filter.includes.is_empty() || !filter.excludes.is_empty() {
                    return Err(unsupported(key));
                }
                hits.source_skip = filter.skip;
            }
            "explain"
            | "version"
            | "seq_no_primary_term"
            | "track_scores"
            | "docvalue_fields"
            | "stored_fields"
            | "fields"
            | "highlight"
            | "script_fields"
            | "collapse"
            | "ignore_unmapped"
            | "inner_hits" => return Err(unsupported(key)),
            _ => return Err(unknown(p, key)),
        }
    }
    Ok(hits)
}
