//! One parsed reader mapping for every opened index with the same mapping text.
//!
//! The table is keyed by the descriptor's escaped mapping text, so a hit skips
//! the unescape and the parse. It holds weak references: a mapping no open
//! index holds is freed.

use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use oss_doc::mapping::TextInterner;

use crate::error::ReaderError;
use crate::mapping::Mapping;

/// A parsed mapping and the JSON source it was parsed from.
#[derive(Debug, PartialEq, Eq)]
pub struct SharedMapping {
    mapping: Mapping,
    json: Arc<str>,
}

impl SharedMapping {
    /// The mapping's JSON source as the document builder reads it; empty when
    /// the descriptor holds none.
    #[must_use]
    pub fn json(&self) -> &str {
        &self.json
    }

    /// The mapping a descriptor without one has.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            mapping: Mapping::default(),
            json: Arc::from(""),
        }
    }

    /// The mapping read from its JSON source, shared with nothing.
    ///
    /// # Errors
    ///
    /// When `json` is not a mapping.
    pub fn from_json(json: &str) -> Result<Self, ReaderError> {
        Ok(Self {
            mapping: Mapping::from_json(json)?,
            json: Arc::from(json),
        })
    }
}

impl Deref for SharedMapping {
    type Target = Mapping;

    fn deref(&self) -> &Mapping {
        &self.mapping
    }
}

fn table() -> &'static TextInterner<SharedMapping> {
    static TABLE: OnceLock<TextInterner<SharedMapping>> = OnceLock::new();
    TABLE.get_or_init(TextInterner::new)
}

/// The mapping of a descriptor's escaped mapping text, shared with every
/// other index of the same text.
///
/// # Errors
///
/// When the text is not a mapping; it is not stored.
pub fn shared_of(escaped: Option<&[u8]>) -> Result<Arc<SharedMapping>, ReaderError> {
    let Some(escaped) = escaped else {
        // ALLOC: the empty mapping, a few words.
        return Ok(Arc::new(SharedMapping::empty()));
    };
    let Ok(text) = std::str::from_utf8(escaped) else {
        // Not text the table can key by: parse it unshared, as before.
        return Ok(Arc::new(SharedMapping::from_json(&Mapping::unescape(
            escaped,
        )?)?));
    };
    table().get_with(text, |t| {
        SharedMapping::from_json(&Mapping::unescape(t.as_bytes())?)
    })
}

/// How many mappings the table holds live.
#[cfg(test)]
pub fn live() -> usize {
    table().live()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escaped(tag: &str, fields: usize) -> String {
        let props: Vec<String> = (0..fields)
            .map(|n| format!(r#"\"{tag}{n}\":{{\"type\":\"keyword\"}}"#))
            .collect();
        format!(r#"{{\"properties\":{{{}}}}}"#, props.join(","))
    }

    // docs/specs/shared-reader-mappings AC-1
    #[test]
    fn equal_text_is_one_mapping_and_unequal_text_is_two() {
        let a = escaped("ac1a", 3);
        let b = escaped("ac1b", 3);
        let first = shared_of(Some(a.as_bytes())).unwrap();
        let again = shared_of(Some(a.as_bytes())).unwrap();
        let other = shared_of(Some(b.as_bytes())).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    // docs/specs/shared-reader-mappings AC-1
    #[test]
    fn a_text_that_does_not_parse_is_an_error() {
        assert!(shared_of(Some(br#"{\"properties\":"#)).is_err());
        assert!(shared_of(Some(br#"{\"properties\":"#)).is_err());
    }

    // docs/specs/shared-reader-mappings AC-2
    #[test]
    fn a_shared_mapping_answers_as_an_unshared_parse() {
        let text = escaped("ac2", 4);
        let shared = shared_of(Some(text.as_bytes())).unwrap();
        let json = Mapping::unescape(text.as_bytes()).unwrap();
        assert_eq!(shared.json(), json);
        assert_eq!(**shared, Mapping::from_json(&json).unwrap());
        assert_eq!(shared.len(), 4);
        assert_eq!(shared_of(None).unwrap().json(), "");
    }

    // docs/specs/shared-reader-mappings AC-3
    #[test]
    fn a_mapping_no_index_holds_is_not_held() {
        let text = escaped("ac3", 2);
        let held = shared_of(Some(text.as_bytes())).unwrap();
        let weak = Arc::downgrade(&held);
        assert!(live() >= 1);
        drop(held);
        assert!(weak.upgrade().is_none());
    }
}
