//! The mapper lookups a document's walk makes.

use crate::mapping::{Mapper, Object};

/// `getMapper`: a child, through the objects a dotted name's leading parts
/// name.
pub(super) fn get_mapper<'m>(obj: Option<&'m Object>, parts: &[&str]) -> Option<&'m Mapper> {
    let mut obj = obj?;
    let (last, leading) = parts.split_last()?;
    for part in leading {
        match obj.child(part) {
            Some(Mapper::Object(o)) => obj = o,
            _ => return None,
        }
    }
    obj.child(last)
}

/// `containsDisabledObjectMapper`.
pub(super) fn contains_disabled(obj: Option<&Object>, parts: &[&str]) -> bool {
    let Some(mut obj) = obj else {
        return false;
    };
    let leading = parts.len().saturating_sub(1);
    for part in parts.iter().take(leading) {
        match obj.child(part) {
            Some(Mapper::Object(o)) => {
                if !o.enabled {
                    return true;
                }
                obj = o;
            }
            _ => break,
        }
    }
    false
}
