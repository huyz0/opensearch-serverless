//! The `_source` of one nested object of a parent's source: what a nested
//! inner hit reports (`FetchPhase.getNextNestedSource` at OpenSearch 3.10.0),
//! the element at `offset` of the array under the nested path, written as a
//! filtered source is (`HashMap` key order).

use std::io::{self, Write};

use super::source_filter::scratch_bytes;
use super::source_map::{Value, read, write_value};
use crate::dsl::tokens::{Kind, Recorder};

/// The value under dotted `path` of `v`, a key holding a dot being tried
/// whole before the path is split at it.
fn lookup<'v, 'a>(v: &'v Value<'a>, path: &str) -> Option<&'v Value<'a>> {
    let Value::Object(entries) = v else {
        return None;
    };
    if let Some((_, found)) = entries.iter().find(|(k, _)| *k == path) {
        return Some(found);
    }
    path.match_indices('.').find_map(|(i, _)| {
        let (head, rest) = (path.get(..i)?, path.get(i.checked_add(1)?..)?);
        let (_, inner) = entries.iter().find(|(k, _)| *k == head)?;
        lookup(inner, rest)
    })
}

/// Writes the object at `offset` under nested `path` of `source`; false, and
/// nothing written, when the source has none (a child of an update that no
/// longer holds it).
///
/// # Errors
/// A source that is not JSON, or a write failure.
pub fn write_nested(
    out: &mut dyn Write,
    source: &[u8],
    path: &str,
    offset: usize,
) -> io::Result<bool> {
    // ALLOC: the source's tokens and text, bounded by the stored source,
    // which the caller reserved with `scratch_bytes`.
    let mut rec = Recorder::new(scratch_bytes(source.len()));
    let mut tokenizer = crate::dsl::source::tokenizer();
    tokenizer
        .feed(source, &mut rec)
        .map_err(|e| io::Error::other(e.into_failure().reason))?;
    tokenizer
        .finish(&mut rec)
        .map_err(|e| io::Error::other(e.into_failure().reason))?;
    let mut p = rec.parser();
    let Some(Kind::StartObject) = p.next_token() else {
        return Err(io::Error::other("a _source that is not a JSON object"));
    };
    let root = read(&mut p, Kind::StartObject);
    let element = match lookup(&root, path) {
        Some(Value::Array(items)) => items.get(offset),
        Some(object @ Value::Object(_)) if offset == 0 => Some(object),
        _ => None,
    };
    let Some(element) = element else {
        return Ok(false);
    };
    // ALLOC: one entry a key of the widest object written.
    let mut order = Vec::new();
    write_value(out, element, &mut order)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(source: &str, path: &str, offset: usize) -> Option<String> {
        let mut out = Vec::new();
        write_nested(&mut out, source.as_bytes(), path, offset)
            .expect("writes")
            .then(|| String::from_utf8(out).expect("utf8"))
    }

    #[test]
    fn the_element_at_the_offset_of_the_array_under_the_path() {
        let s = r#"{"t":"x","c":[{"w":"a"},{"w":"b"}]}"#;
        assert_eq!(nested(s, "c", 0).as_deref(), Some(r#"{"w":"a"}"#));
        assert_eq!(nested(s, "c", 1).as_deref(), Some(r#"{"w":"b"}"#));
        assert_eq!(nested(s, "c", 2), None);
    }

    #[test]
    fn a_single_object_is_its_own_offset_zero_and_a_missing_path_is_none() {
        let s = r#"{"c":{"w":"a"}}"#;
        assert_eq!(nested(s, "c", 0).as_deref(), Some(r#"{"w":"a"}"#));
        assert_eq!(nested(s, "c", 1), None);
        assert_eq!(nested(s, "d", 0), None);
    }

    #[test]
    fn a_dotted_path_walks_objects_or_a_key_with_the_dot() {
        let walked = r#"{"a":{"c":[{"w":1}]}}"#;
        let whole = r#"{"a.c":[{"w":1}]}"#;
        assert_eq!(nested(walked, "a.c", 0).as_deref(), Some(r#"{"w":1}"#));
        assert_eq!(nested(whole, "a.c", 0).as_deref(), Some(r#"{"w":1}"#));
    }
}
