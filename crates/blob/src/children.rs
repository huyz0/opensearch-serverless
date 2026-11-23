//! Listing the children of a prefix with one LIST each: the portable form of
//! `BlobStore::list_children`.

use crate::error::BlobResult;
use crate::store::{BlobStore, ListPage};

/// The child of `prefix` that holds `key`: `key` up to and including its
/// next `/` after `prefix`, or `key` itself when none follows.
fn child_of<'k>(prefix: &str, key: &'k str) -> &'k str {
    let rest = key.get(prefix.len()..).unwrap_or("");
    match rest.find('/') {
        Some(at) => key
            .get(..prefix.len().saturating_add(at).saturating_add(1))
            .unwrap_or(key),
        None => key,
    }
}

/// The key after which no key of `child` is listed: a directory-like child
/// (`x/`) is passed with `x0`, since `0` is the byte after `/`; a plain key
/// is its own cursor.
fn after(child: &str) -> String {
    match child.strip_suffix('/') {
        // ALLOC: one cursor key, rebuilt each step.
        Some(stem) => format!("{stem}0"),
        // ALLOC: as above.
        None => child.to_owned(),
    }
}

/// Visits up to `max_children` children of `prefix` in byte order, starting
/// strictly after the child `start_after`, with one LIST of one key for
/// each; the page is `truncated` when it stopped at `max_children`, which
/// may be the end anyway. A short page paid one LIST more, the one that
/// found the end.
pub(crate) fn list_children<S: BlobStore + ?Sized>(
    store: &S,
    prefix: &str,
    start_after: Option<&str>,
    max_children: usize,
    visit: &mut dyn FnMut(&str),
) -> BlobResult<ListPage> {
    // ALLOC: one cursor key.
    let mut cursor: Option<String> = start_after.map(after);
    let mut seen = 0usize;
    while seen < max_children {
        // ALLOC: one key a step, at most a key's length.
        let mut found: Option<String> = None;
        // LIST: one key a child; the sweep's budget bounds the children.
        store.list_prefix(prefix, cursor.as_deref(), 1, &mut |key, _| {
            // ALLOC: one key, as above.
            found = Some(key.to_owned());
        })?;
        let Some(key) = found else {
            return Ok(ListPage {
                keys: seen,
                truncated: false,
            });
        };
        let child = child_of(prefix, &key);
        visit(child);
        cursor = Some(after(child));
        seen = seen.saturating_add(1);
    }
    Ok(ListPage {
        keys: seen,
        truncated: true,
    })
}
