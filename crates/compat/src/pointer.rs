//! JSON pointers (RFC 6901) with one extension: a `*` segment matches every
//! element of an array and every member of an object.
//!
//! Normalisation rules address the response document `{status, headers,
//! body}` with these pointers, for example `/body/hits/hits/*/_id`.

use std::fmt;

use serde_json::Value;

/// One step of a pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// An object member name, or an array index when it parses as one.
    Key(String),
    /// Every array element or object member.
    Any,
}

/// A parsed pointer. Always addresses something below the root: a pointer
/// to the whole document would let a rule hide every difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    text: String,
    segments: Vec<Segment>,
}

impl Pointer {
    /// Parses `text`, which must start with `/` and name at least one
    /// segment. `~0` and `~1` decode to `~` and `/`; a segment of exactly `*`
    /// is the wildcard.
    pub fn parse(text: &str) -> Result<Self, String> {
        let Some(rest) = text.strip_prefix('/') else {
            return Err(format!("pointer {text:?} must start with '/'"));
        };
        let mut segments = Vec::new();
        for raw in rest.split('/') {
            if raw == "*" {
                segments.push(Segment::Any);
                continue;
            }
            segments.push(Segment::Key(unescape(raw).ok_or_else(|| {
                format!("pointer {text:?} has a '~' not followed by 0 or 1")
            })?));
        }
        Ok(Self {
            text: text.to_owned(),
            segments,
        })
    }

    /// Replaces every value this pointer matches in `doc` with `with`, and
    /// returns how many it matched. A path that does not exist matches
    /// nothing; nothing is created.
    pub fn replace_all(&self, doc: &mut Value, with: &Value) -> usize {
        replace_in(doc, &self.segments, with)
    }
}

impl fmt::Display for Pointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Decodes `~1` then `~0`, in that order, as RFC 6901 requires. `None` when
/// a `~` is followed by anything else.
fn unescape(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '~' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('0') => out.push('~'),
            Some('1') => out.push('/'),
            _ => return None,
        }
    }
    Some(out)
}

/// Escapes one member name for display inside a pointer.
pub(crate) fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn replace_in(node: &mut Value, segments: &[Segment], with: &Value) -> usize {
    let Some((first, rest)) = segments.split_first() else {
        *node = with.clone();
        return 1;
    };
    match (first, node) {
        (Segment::Any, Value::Array(items)) => items
            .iter_mut()
            .map(|item| replace_in(item, rest, with))
            .fold(0, usize::saturating_add),
        (Segment::Any, Value::Object(members)) => members
            .values_mut()
            .map(|member| replace_in(member, rest, with))
            .fold(0, usize::saturating_add),
        (Segment::Key(key), Value::Object(members)) => members
            .get_mut(key)
            .map_or(0, |member| replace_in(member, rest, with)),
        (Segment::Key(key), Value::Array(items)) => key
            .parse::<usize>()
            .ok()
            .and_then(|index| items.get_mut(index))
            .map_or(0, |item| replace_in(item, rest, with)),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn replaced(pointer: &str, mut doc: Value) -> (usize, Value) {
        let n = Pointer::parse(pointer)
            .unwrap()
            .replace_all(&mut doc, &json!("X"));
        (n, doc)
    }

    #[test]
    fn a_plain_pointer_replaces_one_member() {
        let (n, doc) = replaced("/body/took", json!({"body": {"took": 5, "ok": true}}));
        assert_eq!(n, 1);
        assert_eq!(doc, json!({"body": {"took": "X", "ok": true}}));
    }

    #[test]
    fn a_wildcard_covers_every_array_element_and_object_member() {
        let (n, doc) = replaced(
            "/hits/*/_id",
            json!({"hits": [{"_id": "a"}, {"_id": "b"}, {"other": 1}]}),
        );
        assert_eq!(n, 2);
        assert_eq!(
            doc,
            json!({"hits": [{"_id": "X"}, {"_id": "X"}, {"other": 1}]})
        );

        let (n, doc) = replaced(
            "/nodes/*/name",
            json!({"nodes": {"n1": {"name": "a"}, "n2": {"name": "b"}}}),
        );
        assert_eq!(n, 2);
        assert_eq!(
            doc,
            json!({"nodes": {"n1": {"name": "X"}, "n2": {"name": "X"}}})
        );
    }

    #[test]
    fn a_numeric_segment_indexes_an_array() {
        let (n, doc) = replaced("/a/1", json!({"a": [1, 2, 3]}));
        assert_eq!(n, 1);
        assert_eq!(doc, json!({"a": [1, "X", 3]}));
        assert_eq!(replaced("/a/9", json!({"a": [1]})).0, 0);
        assert_eq!(replaced("/a/x", json!({"a": [1]})).0, 0);
    }

    #[test]
    fn a_missing_path_matches_nothing_and_creates_nothing() {
        let doc = json!({"body": {"ok": true}});
        let (n, after) = replaced("/body/took", doc.clone());
        assert_eq!(n, 0);
        assert_eq!(after, doc);
        assert_eq!(replaced("/body/ok/deeper", doc.clone()).0, 0);
        assert_eq!(replaced("/*/x", json!(3)).0, 0);
    }

    #[test]
    fn escapes_decode_to_tilde_and_slash() {
        let (n, doc) = replaced("/a~1b/c~0d", json!({"a/b": {"c~d": 1}}));
        assert_eq!(n, 1);
        assert_eq!(doc, json!({"a/b": {"c~d": "X"}}));
        assert_eq!(escape("a/b~c"), "a~1b~0c");
    }

    #[test]
    fn malformed_pointers_are_refused() {
        assert!(
            Pointer::parse("body/took")
                .unwrap_err()
                .contains("must start with '/'")
        );
        assert!(Pointer::parse("").is_err());
        assert!(Pointer::parse("/a~2").unwrap_err().contains("'~'"));
        assert!(Pointer::parse("/a~").is_err());
    }

    #[test]
    fn display_is_the_original_text() {
        assert_eq!(Pointer::parse("/a~1b/*").unwrap().to_string(), "/a~1b/*");
    }
}
