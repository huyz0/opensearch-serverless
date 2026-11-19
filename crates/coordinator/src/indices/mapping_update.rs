//! What a mapping update does to the stored mapping: the body it needs, the
//! merge over what is stored, whether anything changed, and the status each
//! refusal answers with.

use oss_doc::mapping::{Mapping, MappingError};

use crate::route::resolve::Refusal;

/// How many times the descriptor swap is tried before the update is a conflict.
pub const ATTEMPTS: u32 = 4;

/// What to do with the stored mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingPlan {
    /// The update changes nothing: acknowledge it and write nothing.
    Unchanged,
    /// Store this mapping, as JSON without the `_doc` wrapper.
    Replace(String),
}

/// The HTTP status a mapping refusal of type `ty` answers with.
pub fn status_of(ty: &str) -> u16 {
    match ty {
        "mapper_exception" => 500,
        oss_doc::mapping::UNSUPPORTED => 501,
        _ => 400,
    }
}

fn refusal(error: MappingError) -> Refusal {
    Refusal::new(status_of(error.ty), error.ty, error.reason)
}

/// Merges `body` over the `current` stored mapping.
///
/// # Errors
///
/// A [`Refusal`] for a missing body, a stored mapping or body that does not
/// parse, and a conflict the merge refuses.
pub fn plan(current: Option<&str>, body: Option<&[u8]>) -> Result<MappingPlan, Refusal> {
    let body = body.filter(|b| !b.is_empty());
    let Some(body) = body else {
        return Err(Refusal::new(
            400,
            "missing_body",
            "a mapping body is required",
        ));
    };
    let stored = Mapping::from_descriptor(current).map_err(refusal)?;
    let update = Mapping::parse(body).map_err(refusal)?;
    let merged = stored.merge(&update).map_err(refusal)?.to_json();
    if current.is_some() && merged == stored.to_json() {
        return Ok(MappingPlan::Unchanged);
    }
    Ok(MappingPlan::Replace(merged))
}
