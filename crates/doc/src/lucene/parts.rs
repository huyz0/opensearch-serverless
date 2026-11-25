//! The small pieces of the indexer a document is described by: its metadata,
//! the `_id` term and the mapping's leaf for a field name.

use crate::mapping::{Leaf, Mapper, MappingError, Object};

use super::encode::encode_id;

/// A document's metadata, given by the index request and the engine.
#[derive(Clone, Copy, Debug)]
pub struct Meta<'a> {
    /// The document's id.
    pub id: &'a str,
    /// Its sequence number.
    pub seq_no: i64,
    /// The primary term it was indexed under.
    pub primary_term: i64,
    /// Its version.
    pub version: i64,
    /// Its source, stored as given.
    pub source: &'a [u8],
}

/// `Uid.encodeId`: the bytes of the `_id` term for `id`.
///
/// # Errors
///
/// An empty id.
pub fn uid(id: &str) -> Result<Vec<u8>, MappingError> {
    encode_id(id)
}

/// The leaf at `full`, a full field name, multi-fields included.
pub(super) fn leaf<'m>(root: &'m Object, full: &str) -> Option<&'m Leaf> {
    let mut obj = root;
    let mut rest = full;
    loop {
        let (head, tail) = rest
            .split_once('.')
            .map_or((rest, None), |(h, t)| (h, Some(t)));
        match (obj.child(head)?, tail) {
            (Mapper::Object(o), Some(t)) => {
                obj = o;
                rest = t;
            }
            (Mapper::Leaf(l), None) => return Some(l),
            (Mapper::Leaf(l), Some(t)) => {
                return l
                    .multi_fields()
                    .iter()
                    .find(|(name, _)| name == t)
                    .map(|(_, m)| m);
            }
            (Mapper::Object(_), None) => return None,
        }
    }
}
