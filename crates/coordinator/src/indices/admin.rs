//! The index routes that read and delete, decided without a node: the names
//! refused before any read, the view a GET renders, what a delete refuses and
//! how a failed one is told, the acknowledgement bodies, and the words of an
//! alias that failed after its index was created.

use std::fmt::Write as _;

use oss_doc::mapping::json::write_str;

use crate::route::resolve::Refusal;

/// The prefix of a data stream's backing indices, which a create may not use.
pub const BACKING_INDEX_PREFIX: &str = ".ds-";

/// Which part of an index a GET renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Aliases, mappings and settings.
    Full,
    /// Settings only, or one named setting.
    Settings,
    /// The mapping only.
    Mapping,
}

/// Refuses a name that cannot be an index here: names beginning with an
/// underscore name APIs, and a create may not take a data stream's backing
/// prefix.
///
/// # Errors
///
/// A [`Refusal`] in the shell's words.
pub fn check_name(index: &str, creating: bool) -> Result<(), Refusal> {
    if index.starts_with('_') {
        return Err(Refusal::new(
            501,
            "not_implemented",
            // ALLOC: one message, bounded by the name.
            format!(
                "'{index}' is not an index: names beginning with an underscore are reserved for APIs, and \
this shell does not implement this one. An index name that is not there answers 404; this \
is a 501 because there is nothing here to find under any name"
            ),
        ));
    }
    if creating && index.starts_with(BACKING_INDEX_PREFIX) {
        return Err(Refusal::new(
            400,
            "illegal_argument_exception",
            // ALLOC: one message, bounded by the name.
            format!(
                "index name [{index}] must not start with '{BACKING_INDEX_PREFIX}': that prefix names a data \
stream's backing indices, which its rollover creates"
            ),
        ));
    }
    Ok(())
}

/// The view a GET of `path` renders: settings for a settings path or a named
/// setting, the mapping for a mapping path, else all.
pub fn view_of(path: &str, setting_name: Option<&str>) -> View {
    if path.ends_with("/_settings") || setting_name.is_some() {
        View::Settings
    } else if path.ends_with("/_mapping") || path.ends_with("/_mappings") {
        View::Mapping
    } else {
        View::Full
    }
}

/// Refuses deleting the write index of a data stream.
///
/// # Errors
///
/// A [`Refusal`] naming the stream, when `stream` is one.
pub fn delete_check(index: &str, stream: Option<&str>) -> Result<(), Refusal> {
    stream.map_or(Ok(()), |stream| Err(write_index_refusal(index, stream)))
}

/// The refusal for deleting `index`, the write index of `stream`.
pub fn write_index_refusal(index: &str, stream: &str) -> Refusal {
    Refusal::new(
        400,
        "illegal_argument_exception",
        // ALLOC: one message, bounded by the names.
        format!(
            "index [{index}] is the write index for data stream [{stream}] and cannot be deleted"
        ),
    )
}

/// The refusal for a delete that deleted nothing: a conflict when the index
/// was there to delete, not found when it was not.
pub fn delete_failed(index: &str, existed: bool) -> Refusal {
    if existed {
        Refusal::new(
            409,
            "version_conflict_engine_exception",
            // ALLOC: one message, bounded by the name.
            format!("index [{index}] changed concurrently; nothing was deleted, retry"),
        )
    } else {
        no_such_index(index)
    }
}

/// The 404 for an index that is not there.
pub fn no_such_index(index: &str) -> Refusal {
    // ALLOC: one message, bounded by the name.
    Refusal::new(404, "index_not_found", format!("no such index: {index}"))
}

/// The body of a successful create.
pub fn create_ack(index: &str, shards: u32) -> String {
    // ALLOC: one small body, bounded by the name.
    let mut out = String::from(r#"{"acknowledged":true,"shards_acknowledged":true,"index":"#);
    write_str(&mut out, index);
    let _ = write!(out, r#","shards":{shards}}}"#);
    out
}

/// The body of a successful delete.
pub fn delete_ack(index: &str) -> String {
    // ALLOC: one small body, bounded by the name.
    let mut out = String::from(r#"{"acknowledged":true,"index":"#);
    write_str(&mut out, index);
    out.push('}');
    out
}

/// An alias refusal after its index was created, said so that the caller
/// knows the index is there.
pub fn alias_failed_after_create(index: &str, alias: &str, refused: &Refusal) -> Refusal {
    Refusal::new(
        refused.status,
        refused.kind,
        // ALLOC: one message, bounded by the names and the refusal's text.
        format!(
            "index [{index}] was created, and its alias [{alias}] was not: {}",
            refused.reason
        ),
    )
}
