//! The `stored_fields` request key's check against the mapping: a name that is
//! an object is OpenSearch's "isn't a leaf field" error, and a field mapped
//! `store: true` is one this node keeps no stored value for.

use crate::dsl::queries::build::simple_match;
use crate::failure::Failure;
use crate::mapping::Mapping;
use crate::search::ShardError;

fn bad_request(reason: String) -> ShardError {
    ShardError {
        status: 400,
        failure: Failure::plain("illegal_argument_exception", reason),
    }
}

/// Each name of `stored_fields` over `mapping`.
///
/// # Errors
///
/// OpenSearch's refusal of an object name, or this node's for a name or
/// pattern that matches a field mapped `store: true`.
pub fn check(names: &[String], mapping: &Mapping) -> Result<(), ShardError> {
    for name in names {
        if name == "_source" || name == "_none_" {
            continue;
        }
        if mapping.is_object(name) {
            // ALLOC: error path; one message.
            return Err(bad_request(format!("field [{name}] isn't a leaf field")));
        }
        let stored = mapping.names().any(|n| {
            (n == name || (name.contains('*') && simple_match(name, n)))
                && mapping.get(n).is_some_and(|f| f.fetch.stored)
        });
        if stored {
            // ALLOC: error path; one message.
            return Err(bad_request(format!(
                "[stored_fields] of the stored field [{name}] is not supported by this node yet"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "stored_fields_tests.rs"]
mod tests;
